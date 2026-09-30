//! Passwords: the strength every new one must have, and the PHC string stored in its place
//! (`requirements/users.md` §3, `design/database.md`).

use std::sync::LazyLock;
use std::thread;

use argon2::Argon2;
use argon2::password_hash::PasswordHasher;
use tokio::sync::Semaphore;
use zxcvbn::{Score, zxcvbn};

use super::{Code, Problem};

/// zxcvbn's own threshold: below it, a password is guessable offline in hours.
const MIN_SCORE: Score = Score::Three;

/// zxcvbn's work grows faster than the password, so it scores only this many characters. A
/// longer password is at least as strong as its start, so this never lets a weak one through,
/// and there is still no maximum length.
const SCORED_CHARS: usize = 100;

/// Each hash holds Argon2's 19 MiB for its whole run, so at most one per core runs at once,
/// however many requests arrive.
static HASHING: LazyLock<Semaphore> = LazyLock::new(|| {
    Semaphore::new(thread::available_parallelism().map_or(1, |cores| cores.get()))
});

/// Accepts a password that is hard to guess, judged without composition rules. A common or
/// breached password, or one built from `inputs` such as the username, is `weak_password`.
fn check_strength(password: &str, inputs: &[&str]) -> Result<(), Problem> {
    let end = password.char_indices().nth(SCORED_CHARS).map_or(password.len(), |(at, _)| at);
    let entropy = zxcvbn(&password[..end], inputs);
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
    let permit = HASHING.acquire().await.expect("the semaphore is never closed");
    tokio::task::spawn_blocking(move || {
        // Held by the work itself, which runs to the end even if the request is dropped.
        let _permit = permit;
        let inputs: Vec<&str> = inputs.iter().map(String::as_str).collect();
        check_strength(&password, &inputs)?;
        let hash = Argon2::default().hash_password(password.as_bytes());
        hash.map(|hash| hash.to_string()).map_err(|error| {
            tracing::error!(%error, "cannot hash a password");
            Problem::new(Code::Internal)
        })
    })
    .await
    .map_err(|_| Problem::new(Code::Internal))?
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use argon2::password_hash::PasswordVerifier;

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

    #[test]
    fn a_very_long_password_is_quick_to_check() {
        let long = "correct horse battery staple ".repeat(10_000);
        let start = Instant::now();
        assert!(check_strength(&long, &[]).is_ok());
        assert!(start.elapsed() < Duration::from_secs(1), "{:?}", start.elapsed());
    }

    #[tokio::test]
    async fn hashes_to_a_phc_string_the_password_verifies_against() {
        let phc = hash_new("correct horse battery staple".into(), vec![]).await.unwrap();
        assert!(phc.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"), "{phc}");
        let argon2 = Argon2::default();
        assert!(argon2.verify_password(b"correct horse battery staple", phc.as_str()).is_ok());
        assert!(argon2.verify_password(b"correct horse battery stapler", phc.as_str()).is_err());
    }

    #[tokio::test]
    async fn a_weak_password_is_not_hashed() {
        let problem = hash_new("password".into(), vec![]).await.unwrap_err();
        assert_eq!(serde_json::to_value(problem).unwrap()["code"], "weak_password");
    }
}
