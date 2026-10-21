//! Background work at low priority (`requirements/performance.md` §5).

/// How much background work is lowered, in nice steps.
#[cfg(target_os = "linux")]
const BACKGROUND_NICE: i32 = 10;

/// Lower the calling thread's priority by 10 nice steps, for a thread that does only background
/// work, such as analysis.
///
/// On Linux, niceness belongs to a thread rather than the process (`setpriority(2)`, under
/// BUGS), so this lowers only the calling thread, and every child it spawns afterwards, such as
/// ffmpeg, inherits the lowered value. Call it only on a thread the caller owns, never on a
/// pooled one such as tokio's `spawn_blocking`, whose later tasks would inherit it. A failure is
/// logged, and the thread carries on at its priority.
#[cfg(target_os = "linux")]
pub fn lower_thread_priority() {
    if let Err(error) = rustix::process::nice(BACKGROUND_NICE) {
        tracing::warn!(%error, "could not lower a background thread's priority");
    }
}

/// Does nothing off Linux, where niceness belongs to the whole process, so lowering it would
/// lower request handling too.
#[cfg(not(target_os = "linux"))]
pub fn lower_thread_priority() {
    tracing::debug!("background threads keep their priority off Linux");
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use std::process::Command;

    use rustix::process::getpriority_process;

    use super::*;

    /// The nice value of a child spawned from the calling thread: the 19th field of its
    /// `/proc/self/stat`, after the name in parentheses, which may hold spaces.
    fn childs_nice() -> i32 {
        let out = Command::new("cat").arg("/proc/self/stat").output().unwrap();
        let stat = String::from_utf8(out.stdout).unwrap();
        stat.rsplit_once(") ").unwrap().1.split(' ').nth(16).unwrap().parse().unwrap()
    }

    #[test]
    fn lowers_the_calling_thread_and_its_children_only() {
        let before = getpriority_process(None).unwrap();
        let lowered = (before + BACKGROUND_NICE).min(19);
        let background = std::thread::spawn(|| {
            lower_thread_priority();
            (getpriority_process(None).unwrap(), childs_nice())
        });
        assert_eq!(background.join().unwrap(), (lowered, lowered));
        assert_eq!((getpriority_process(None).unwrap(), childs_nice()), (before, before));
    }
}
