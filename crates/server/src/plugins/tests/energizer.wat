;; A plugin written by hand for the server's tests, which writes a field of its own into files as a
;; tagger does. On any event it replaces `Album/01.wav` in root 1 with a WAV of 80 silent samples
;; whose ID3v2 tag holds `TXXX:ENERGY` as `High`, the field its manifest adds a tag type for.
;; Then it answers "done".
(component $plugin
  ;; The types `handle` takes, as `wit/plugin.wit` declares them. Each is exported, so it has
  ;; the name the component model requires of a record, variant, or enum in an export.
  (type $track-type (record
    (field "id" u64)
    (field "title" string)
    (field "artists" (list string))
    (field "album-id" u64)
    (field "album" (option string))
    (field "disc-number" u32)
    (field "track-number" (option u32))
    (field "release-date" (option string))
    (field "isrc" (option string))
    (field "duration-ms" u64)
    (field "has-lyrics" bool)
    (field "root" u64)
    (field "path" string)))
  (export $track "track" (type $track-type))
  (type $changes-type (record (field "changed" (list u64)) (field "removed" (list u64))))
  (export $changes "changes" (type $changes-type))
  (type $scan-finished-type (record
    (field "files-seen" u32)
    (field "added" u32)
    (field "updated" u32)
    (field "moved" u32)
    (field "missing" u32)
    (field "problems" u32)))
  (export $scan-finished "scan-finished" (type $scan-finished-type))
  (type $play-end-type (enum "finished" "skipped" "stopped"))
  (export $play-end "play-end" (type $play-end-type))
  (type $play-type (record
    (field "track" $track)
    (field "started-at-ms" u64)
    (field "listen-time-ms" u64)
    (field "end" $play-end)))
  (export $play "play" (type $play-type))
  (type $playing-type (record (field "track" $track) (field "started-at-ms" u64)))
  (export $playing "playing" (type $playing-type))
  (type $result-kind-type (enum "track" "album" "artist"))
  (export $result-kind "result-kind" (type $result-kind-type))
  (type $selected-type (record (field "kind" $result-kind) (field "id" u64)))
  (export $selected "selected" (type $selected-type))
  (type $found-type (record (field "tracks" u32) (field "albums" u32) (field "artists" u32)))
  (export $found "found" (type $found-type))
  (type $search-type (record
    (field "id" u64)
    (field "user" u64)
    (field "query" string)
    (field "found" $found)
    (field "selected" (option $selected))
    (field "searched-at-ms" u64)))
  (export $search "search" (type $search-type))
  (type $forgotten-type (record (field "user" u64) (field "searches" (option (list u64)))))
  (export $forgotten "forgotten" (type $forgotten-type))
  (type $event-type (variant
    (case "run")
    (case "tracks-changed" $changes)
    (case "scan-finished" $scan-finished)
    (case "scheduled")
    (case "check-settings")
    (case "played" (list $play))
    (case "playing" $playing)
    (case "searched" (list $search))
    (case "searches-forgotten" $forgotten)
    (case "albums-changed" $changes)
    (case "artists-changed" $changes)))
  (export $event "event" (type $event-type))
  (type $error-type (variant
    (case "not-granted")
    (case "refused" string)
    (case "failed" string)
    (case "rate-limited" u64)
    (case "timed-out")))
  (type $write-mode-type (enum "create" "replace"))
  (export $write-mode "write-mode" (type $write-mode-type))
  (import "jewelcase:plugin/files@0.1.0" (instance $files
    (alias outer $plugin $error-type (type $error-def))
    (export "error" (type $error (eq $error-def)))
    (alias outer $plugin $write-mode-type (type $write-mode-def))
    (export "write-mode" (type $mode (eq $write-mode-def)))
    (export "write" (func
      (param "root" u64)
      (param "path" string)
      (param "contents" (list u8))
      (param "mode" $mode)
      (result (result (error $error)))))))

  ;; The memory, and where the host puts what it hands the plugin: the event, then an error's
  ;; words, each at 1024, as the plugin reads neither.
  (core module $heap
    (memory (export "memory") 1)
    (func (export "realloc") (param i32 i32 i32 i32) (result i32)
      (i32.const 1024)))
  (core instance $heap (instantiate $heap))
  (core func $write (canon lower (func $files "write")
    (memory (core memory $heap "memory"))
    (realloc (core func $heap "realloc"))))

  (core module $code
    (import "heap" "memory" (memory 1))
    (import "files" "write" (func $write (param i64 i32 i32 i32 i32 i32 i32)))
    ;; The path at 16. The write's answer goes at 32. The plugin's at 64: `ok`, then a pointer
    ;; to 80 and a length of 4; and at 80, its string.
    (data (i32.const 16) "Album/01.wav")
    (data (i32.const 64) "\00\00\00\00\50\00\00\00\04\00\00\00")
    (data (i32.const 80) "done")
    ;; The file's 244 bytes at 4096, past what the host hands over: its RIFF header and format,
    ;; then 160 bytes of silence, which the memory starts as, then its ID3v2.4 tag in an `ID3 `
    ;; chunk, with the one frame `TXXX`, UTF-8, `ENERGY`, `High`.
    (data (i32.const 4096)
      "RIFF\ec\00\00\00WAVEfmt \10\00\00\00\01\00\01\00@\1f\00\00\80>\00\00\02\00\10\00"
      "data\a0\00\00\00")
    (data (i32.const 4300)
      "ID3 \20\00\00\00ID3\04\00\00\00\00\00\16TXXX\00\00\00\0c\00\00\03ENERGY\00High")
    (func (export "handle") (param i32) (result i32)
      ;; Mode 1 is replace.
      (call $write (i64.const 1) (i32.const 16) (i32.const 12) (i32.const 4096) (i32.const 244)
        (i32.const 1) (i32.const 32))
      (i32.const 64)))
  (core instance $code (instantiate $code
    (with "heap" (instance $heap))
    (with "files" (instance (export "write" (func $write))))))

  (func $handle (param "event" $event) (result (result string (error string)))
    (canon lift (core func $code "handle")
      (memory (core memory $heap "memory"))
      (realloc (core func $heap "realloc"))))
  (export "handle" (func $handle))
)
