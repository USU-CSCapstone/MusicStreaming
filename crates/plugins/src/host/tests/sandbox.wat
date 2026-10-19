;; A plugin written by hand, for testing the sandbox. It fits the plugin world and imports only
;; the WASI clock and poll it needs to wait. By event:
;; - `run` spins forever;
;; - `scheduled` waits 500 ms in a host call, does a little work, and answers "done";
;; - `searches-forgotten` does the same, but waits 10 minutes, past the host's time limit;
;; - `check-settings` grows its memory by 64 MiB, past the host's limit, and traps if refused;
;; - `scan-finished` grows a table by a million elements, past the host's limit, and traps if
;;   refused;
;; - `played` subscribes to the clock 10,000 times without letting go, filling the host's
;;   resource table, and answers "done" if it was not stopped;
;; - any other event answers "done".
(component $plugin
  (import "wasi:io/poll@0.2.0" (instance $poll
    (export "pollable" (type (sub resource)))
    (export "[method]pollable.block" (func (param "self" (borrow 0))))
  ))
  (alias export $poll "pollable" (type $pollable))
  (import "wasi:clocks/monotonic-clock@0.2.0" (instance $clock
    (alias outer $plugin $pollable (type))
    (export "pollable" (type (eq 0)))
    (export "subscribe-duration" (func (param "when" u64) (result (own 1))))
  ))

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
  (type $changes (record (field "changed" (list u64)) (field "removed" (list u64))))
  (export $tracks-changed "tracks-changed" (type $changes))
  (export $albums-changed "albums-changed" (type $changes))
  (export $artists-changed "artists-changed" (type $changes))
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
    (field "started-at" u64)
    (field "listen-time-ms" u64)
    (field "end" $play-end)))
  (export $play "play" (type $play-type))
  (type $playing-type (record (field "track" $track) (field "started-at" u64)))
  (export $playing "playing" (type $playing-type))
  (type $result-kind-type (enum "track" "album" "artist"))
  (export $result-kind "result-kind" (type $result-kind-type))
  (type $selected-type (record (field "kind" $result-kind) (field "id" u64)))
  (export $selected "selected" (type $selected-type))
  (type $found-type (record (field "tracks" u32) (field "albums" u32) (field "artists" u32)))
  (export $found "found" (type $found-type))
  (type $search-type (record
    (field "id" u64)
    (field "query" string)
    (field "found" $found)
    (field "selected" (option $selected))
    (field "searched-at" u64)))
  (export $search "search" (type $search-type))
  (type $event-type (variant
    (case "run")
    (case "tracks-changed" $tracks-changed)
    (case "scan-finished" $scan-finished)
    (case "scheduled")
    (case "check-settings")
    (case "played" (list $play))
    (case "playing" $playing)
    (case "searched" (list $search))
    (case "searches-forgotten" (option (list u64)))
    (case "albums-changed" $albums-changed)
    (case "artists-changed" $artists-changed)))
  (export $event "event" (type $event-type))

  (core func $subscribe (canon lower (func $clock "subscribe-duration")))
  (core func $block (canon lower (func $poll "[method]pollable.block")))

  (core module $code
    (import "wasi" "subscribe-duration" (func $subscribe (param i64) (result i32)))
    (import "wasi" "block" (func $block (param i32)))
    (memory (export "memory") 1)
    (table $refs 1 funcref)
    ;; The answer at 8: `ok`, then a pointer to 32 and a length of 4; and at 32, its string.
    (data (i32.const 8) "\00\00\00\00\20\00\00\00\04\00\00\00")
    (data (i32.const 32) "done")

    ;; For the events these tests send, the host allocates only the event, once per run, and at
    ;; most an empty list, which takes no room, so each always goes at 64.
    (func (export "realloc") (param i32 i32 i32 i32) (result i32)
      (i32.const 64))

    (func (export "handle") (param $event i32) (result i32)
      (local $work i32)
      (local $nanos i64)
      (local.set $nanos (i64.const 500_000_000))
      (block $answer
        (block $grow
          (block $wait
            (block $linger
              (block $hoard
                (block $grow-table
                  (block $spin
                    ;; By the event's case: run, tracks-changed, scan-finished, scheduled,
                    ;; check-settings, played, playing, searched, searches-forgotten, and any
                    ;; other.
                    (br_table $spin $answer $grow-table $wait $grow $hoard $answer $answer $linger
                      $answer (i32.load8_u (local.get $event))))
                  ;; run
                  (loop $forever (br $forever)))
                ;; scan-finished
                (if (i32.eq (table.grow $refs (ref.null func) (i32.const 1_000_000)) (i32.const -1))
                  (then unreachable))
                (br $answer))
              ;; played: each subscription holds two entries, the deadline and its pollable.
              (loop $more
                (drop (call $subscribe (i64.const 0)))
                (local.set $work (i32.add (local.get $work) (i32.const 1)))
                (br_if $more (i32.lt_u (local.get $work) (i32.const 10_000))))
              (br $answer))
            ;; searches-forgotten
            (local.set $nanos (i64.const 600_000_000_000)))
          ;; scheduled and searches-forgotten
          (call $block (call $subscribe (local.get $nanos)))
          ;; Wasmtime checks the epoch at each loop, so an overdue deadline would stop the
          ;; plugin here.
          (loop $more
            (local.set $work (i32.add (local.get $work) (i32.const 1)))
            (br_if $more (i32.lt_u (local.get $work) (i32.const 1000))))
          (br $answer))
        ;; check-settings
        (if (i32.eq (memory.grow (i32.const 1024)) (i32.const -1))
          (then unreachable)))
      (i32.const 8))
  )

  (core instance $wasi
    (export "subscribe-duration" (func $subscribe))
    (export "block" (func $block)))
  (core instance $instance (instantiate $code (with "wasi" (instance $wasi))))
  (func $handle (param "event" $event) (result (result string (error string)))
    (canon lift (core func $instance "handle")
      (memory (core memory $instance "memory"))
      (realloc (core func $instance "realloc"))))
  (export "handle" (func $handle))
)
