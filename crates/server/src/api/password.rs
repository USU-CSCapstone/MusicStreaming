//! Passwords: the strength every new one must have, the PHC string stored in its place, and
//! checking one against it (`requirements/users.md` §3, `design/database.md`).

use std::sync::LazyLock;
use std::thread;

use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use tokio::sync::Semaphore;
use zxcvbn::{Score, zxcvbn};

use super::{Code, Problem};

/// zxcvbn's own threshold: below it, a password is guessable offline in hours.
const MIN_SCORE: Score = Score::Three;

/// Each hash holds Argon2's 19 MiB and a core for its whole run, so hashes run on at most half
/// the cores at once ([`permits`]), however many requests arrive, and the rest are left for
/// playback.
static HASHING: LazyLock<Semaphore> = LazyLock::new(|| {
    Semaphore::new(thread::available_parallelism().map_or(1, |cores| permits(cores.get())))
});

/// How many hashes [`HASHING`] lets run at once on `cores` cores: half of them, and at least one.
fn permits(cores: usize) -> usize {
    (cores / 2).max(1)
}

/// What an unknown account's password is checked against, so refusing it takes as long as
/// refusing a wrong password, and timing does not tell whether an account exists. A hash of
/// `no account has this password` at the default parameters, written out so that even the
/// first refusal costs one check and nothing more.
const DUMMY: &str = concat!(
    "$argon2id$v=19$m=19456,t=2,p=1",
    "$FFn8kmUDtHswtAkjTZH/Fw",
    "$by2VjfK2WKbndNP5YbAN24koouEvgbuj1/Rbl96PFpc",
);

/// Accepts a password that is hard to guess, judged without composition rules. A common or
/// breached password, or one built from `inputs` such as the username, is `weak_password`.
fn check_strength(password: &str, inputs: &[&str]) -> Result<(), Problem> {
    let entropy = zxcvbn(password, inputs);
    if entropy.score() >= MIN_SCORE {
        return Ok(());
    }
    // The warning names the pattern found, such as "This is a very common password"; it never
    // repeats the password.
    let detail = match entropy.feedback().and_then(|feedback| feedback.warning()) {
        Some(warning) => format!("{warning} Choose a longer or less predictable password."),
        None => "Choose a longer or less predictable password.".to_owned(),
    };
    Err(Problem::new(Code::WeakPassword).detail(detail))
}

/// Checks the strength of a new password against `inputs`, then hashes it with Argon2id at the
/// crate's defaults, which are OWASP's, returning the PHC string to store. Both are CPU-bound,
/// so they run off the async threads.
pub async fn hash_new(password: String, inputs: Vec<String>) -> Result<String, Problem> {
    off_thread(move || {
        let inputs: Vec<&str> = inputs.iter().map(String::as_str).collect();
        check_strength(&password, &inputs)?;
        let hash = Argon2::default().hash_password(password.as_bytes());
        hash.map(|hash| hash.to_string()).map_err(|error| {
            tracing::error!(%error, "cannot hash a password");
            Problem::new(Code::Internal)
        })
    })
    .await
}

/// Whether `password` matches the `stored` PHC string. With none stored, or one that cannot be
/// read, it is checked against a dummy hash and never matches, in the same time.
pub async fn verify(password: String, stored: Option<String>) -> Result<bool, Problem> {
    off_thread(move || {
        let stored = stored.as_deref().and_then(|stored| PasswordHash::new(stored).ok());
        let known = stored.is_some();
        let hash = stored
            .unwrap_or_else(|| PasswordHash::new(DUMMY).expect("the dummy hash is well formed"));
        let matches = Argon2::default().verify_password(password.as_bytes(), &hash).is_ok();
        Ok(known && matches)
    })
    .await
}

/// Runs CPU-bound password work off the async threads, as many at once as [`HASHING`] allows.
/// Fails with an internal error if the work doesn't finish.
async fn off_thread<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, Problem> + Send + 'static,
) -> Result<T, Problem> {
    let permit = HASHING.acquire().await.expect("the semaphore is never closed");
    tokio::task::spawn_blocking(move || {
        // Held by the work itself, which runs to the end even if the request is dropped.
        let _permit = permit;
        work()
    })
    .await
    .map_err(|_| Problem::new(Code::Internal))?
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn rejects_guessable_passwords_without_composition_rules() {
        for weak in ["password", "Password123!", "aaaaaaaaaaaaaaaa", "qwertyuiop", "sam-2026"] {
            assert!(check_strength(weak, &["sam"]).is_err(), "{weak}");
        }
        for strong in ["correct horse battery staple", "vqp-mfrk-ztlw", "Ὀδυσσεὺς ἐν Ἰθάκῃ ἦν"]
        {
            assert!(check_strength(strong, &["sam"]).is_ok(), "{strong}");
        }
    }

    #[test]
    fn the_username_makes_a_password_weaker() {
        assert!(check_strength("jewelcaseowner", &[]).is_ok());
        assert!(check_strength("jewelcaseowner", &["jewelcaseowner"]).is_err());
    }

    /// zxcvbn's work grows faster than the password, so it scores only the first 100 characters
    /// itself; this guards that cap.
    #[test]
    fn a_very_long_password_is_quick_to_check() {
        let long = "correct horse battery staple ".repeat(10_000);
        let start = Instant::now();
        assert!(check_strength(&long, &[]).is_ok());
        // Well under a second when idle; the margin is for a loaded machine.
        assert!(start.elapsed() < Duration::from_secs(10), "{:?}", start.elapsed());
    }

    #[tokio::test]
    async fn hashes_to_a_phc_string_the_password_verifies_against() {
        let phc = hash_new("correct horse battery staple".into(), vec![]).await.unwrap();
        assert!(phc.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"), "{phc}");
        let check = |password: &str| verify(password.into(), Some(phc.clone()));
        assert!(check("correct horse battery staple").await.unwrap());
        assert!(!check("correct horse battery stapler").await.unwrap());
    }

    #[tokio::test]
    async fn nothing_matches_a_missing_or_unreadable_hash() {
        for stored in [None, Some(String::new()), Some("$argon2id$garbage".to_owned())] {
            assert!(!verify(String::new(), stored.clone()).await.unwrap(), "{stored:?}");
            assert!(!verify("no account has this password".into(), stored).await.unwrap());
        }
    }

    /// The dummy is a hash of its phrase with the same algorithm, version and parameters as a
    /// new password's hash, so checking it costs the same as checking a stored one.
    #[tokio::test]
    async fn the_dummy_hash_is_read_and_checked_like_a_real_one() {
        let dummy = PasswordHash::new(DUMMY).unwrap();
        let phc = hash_new("correct horse battery staple".into(), vec![]).await.unwrap();
        let real = PasswordHash::new(&phc).unwrap();
        assert_eq!(
            (dummy.algorithm, dummy.version, &dummy.params),
            (real.algorithm, real.version, &real.params)
        );
        let argon2 = Argon2::default();
        assert!(argon2.verify_password(b"no account has this password", &dummy).is_ok());
        assert!(argon2.verify_password(b"correct horse battery staple", &dummy).is_err());
    }

    #[test]
    fn hashing_takes_half_the_cores_and_at_least_one() {
        let permits = [1, 2, 4, 16].map(permits);
        assert_eq!(permits, [1, 1, 2, 8]);
    }

    #[tokio::test]
    async fn a_weak_password_is_not_hashed() {
        let problem = hash_new("password".into(), vec![]).await.unwrap_err();
        assert_eq!(serde_json::to_value(problem).unwrap()["code"], "weak_password");
    }
}
