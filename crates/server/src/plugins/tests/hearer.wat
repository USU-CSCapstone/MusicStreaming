;; A plugin written by hand for the server's tests, which hears likes and hides. To a `marked`
;; event it answers the title of the first song the event names, so a test sees the marks reach
;; the plugin whole; to any other event, and to a `marked` with none, it answers "done".
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
  (type $mark-kind-type (enum "liked" "unliked" "hidden" "unhidden"))
  (export $mark-kind "mark-kind" (type $mark-kind-type))
  (type $mark-type (record (field "track" $track) (field "kind" $mark-kind) (field "at-ms" u64)))
  (export $mark "mark" (type $mark-type))
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
    (case "artists-changed" $changes)
    (case "marked" (list $mark))))
  (export $event "event" (type $event-type))

  (core module $code
    (memory (export "memory") 1)
    ;; Where the host may put what it hands the plugin next: the event, then its lists and
    ;; strings. Nothing is given back, since a run is short.
    (global $next (mut i32) (i32.const 1024))
    (func (export "realloc") (param i32 i32) (param $align i32) (param $size i32) (result i32)
      (local $at i32)
      (local.set $at
        (i32.and
          (i32.add (global.get $next) (i32.sub (local.get $align) (i32.const 1)))
          (i32.sub (i32.const 0) (local.get $align))))
      (global.set $next (i32.add (local.get $at) (local.get $size)))
      (local.get $at))
    ;; The answer at 64: `ok`, then a pointer to 80 and a length of 4; and at 80, its string.
    (data (i32.const 64) "\00\00\00\00\50\00\00\00\04\00\00\00")
    (data (i32.const 80) "done")
    (func (export "handle") (param $event i32) (result i32)
      (local $first i32)
      ;; `marked` is the event's case 11. Its payload, aligned to 8 as a track's u64s are, is
      ;; the list's pointer and length.
      (if (i32.and
            (i32.eq (i32.load8_u (local.get $event)) (i32.const 11))
            (i32.ne (i32.load offset=12 (local.get $event)) (i32.const 0)))
        (then
          ;; A mark starts with its track, whose title, after its u64 id, is a pointer and a
          ;; length at 8. The answer points at it.
          (local.set $first (i32.load offset=8 (local.get $event)))
          (i32.store offset=68 (i32.const 0) (i32.load offset=8 (local.get $first)))
          (i32.store offset=72 (i32.const 0) (i32.load offset=12 (local.get $first)))))
      (i32.const 64)))
  (core instance $code (instantiate $code))

  (func $handle (param "event" $event) (result (result string (error string)))
    (canon lift (core func $code "handle")
      (memory (core memory $code "memory"))
      (realloc (core func $code "realloc"))))
  (export "handle" (func $handle))
)
