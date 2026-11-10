//! A plugin for the end-to-end tests. Run now counts the tracks and saves a note into the first
//! root of the library; saving settings is refused for the token `wrong-token`. It makes no
//! network calls.

#[cfg(target_arch = "wasm32")]
mod plugin {
    use std::time::{SystemTime, UNIX_EPOCH};

    // Reads the contract from the host's copy (plugins/README.md).
    wit_bindgen::generate!({ path: "../../../crates/plugins/wit", world: "plugin" });

    use jewelcase::plugin::files::{self, WriteMode};
    use jewelcase::plugin::settings::{self, Value};
    use jewelcase::plugin::{host, library};

    struct Plugin;

    /// Counts the tracks, all on one page in the tests' small library, then saves a note named
    /// for the time, so no run finds it taken.
    fn run() -> Result<String, String> {
        let count = library::tracks(None, 500).map_err(|e| format!("{e:?}"))?.len();
        host::log(&format!("Counted {count} tracks"));
        let root = files::roots().map_err(|e| format!("{e:?}"))?.first().copied();
        let root = root.ok_or("The library has no folder to save into.")?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis());
        let name = format!("e2e-test-plugin-note-{now}.txt");
        files::write(root, &name, b"Saved by the test plugin.\n", WriteMode::Create)
            .map_err(|e| format!("{e:?}"))?;
        host::log(&format!("Saved {name}"));
        Ok(format!("Saved a note beside {count} tracks."))
    }

    impl Guest for Plugin {
        /// Runs on a run request, refuses the token `wrong-token` when settings are checked, and
        /// does nothing for any other event.
        fn handle(event: Event) -> Result<String, String> {
            match event {
                Event::Run => run(),
                Event::CheckSettings => match settings::get("token") {
                    Some(Value::Text(token)) if token == "wrong-token" => {
                        Err("The test plugin refused this token.".into())
                    }
                    _ => Ok("Settings accepted.".into()),
                },
                _ => Ok("Nothing to do.".into()),
            }
        }
    }

    export!(Plugin);
}
