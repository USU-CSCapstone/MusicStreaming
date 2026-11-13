use super::*;

fn s(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| n.to_string()).collect()
}

#[test]
fn a_downloaded_videos_title_is_read() {
    let read =
        read("Travis Scott feat. Future, Young Thug & M.I.A. - FRANCHISE (REMIX - Official Audio)");
    assert_eq!(read.title, "FRANCHISE (REMIX)");
    assert_eq!(read.artist.as_deref(), Some("Travis Scott"));
    assert_eq!(read.featured, s(&["Future", "Young Thug", "M.I.A."]));
    assert_eq!(read.artists(), s(&["Travis Scott", "Future", "Young Thug", "M.I.A."]));
    assert_eq!(read.search_artist(), Some("Travis Scott"));
}

#[test]
fn noise_in_brackets_or_after_a_dash_goes() {
    assert_eq!(read("Song (Official Audio)").title, "Song");
    assert_eq!(read("Song [Official Music Video]").title, "Song");
    assert_eq!(read("Song (Lyrics) [HD]").title, "Song");
    assert_eq!(read("A - Song - Official Video").title, "Song");
    assert_eq!(read("Song (Radio Edit)").title, "Song (Radio Edit)", "a version is kept");
    assert_eq!(read("Song [Live at Wembley - HD]").title, "Song [Live at Wembley]");
}

#[test]
fn featured_artists_in_the_title_are_found() {
    let read = read("Kendrick Lamar - Money Trees (feat. Jay Rock)");
    assert_eq!(read.title, "Money Trees");
    assert_eq!(read.artists(), s(&["Kendrick Lamar", "Jay Rock"]));
    let both = super::read("A ft. B - Song (featuring C and D)");
    assert_eq!(both.artists(), s(&["A", "B", "C", "D"]));
    let once = super::read("A feat. B - Song (feat. b)");
    assert_eq!(once.artists(), s(&["A", "B"]), "named twice, kept once");
}

#[test]
fn a_main_artist_is_never_split() {
    let read = read("Simon & Garfunkel - The Boxer");
    assert_eq!(read.artists(), s(&["Simon & Garfunkel"]));
    assert_eq!(read.search_artist(), Some("Simon"));
    assert_eq!(super::read("Earth, Wind & Fire - September").artists(), s(&["Earth, Wind & Fire"]));
}

#[test]
fn a_title_with_no_artist_names_none() {
    let read = read("Karma Police");
    assert_eq!(read, Read { title: "Karma Police".into(), ..Read::default() });
    assert!(read.artists().is_empty());
    assert_eq!(read.search_artist(), None);
    assert_eq!(super::read(" - Karma Police").artist, None);
}

#[test]
fn a_leading_track_number_is_dropped() {
    assert_eq!(read("06 - Karma Police").title, "Karma Police");
    assert_eq!(read("06. Karma Police").title, "Karma Police");
    assert_eq!(read("06_Karma Police").title, "Karma Police");
    assert_eq!(read("1979").title, "1979", "a title that is a number");
    assert_eq!(read("99 Luftballons").title, "99 Luftballons");
    assert_eq!(read("06 - Radiohead - Karma Police").artist.as_deref(), Some("Radiohead"));
}

#[test]
fn unclosed_brackets_are_left_alone() {
    assert_eq!(read("Song (unfinished").title, "Song (unfinished");
}

#[test]
fn a_guess_is_the_title_and_the_artists() {
    let tags =
        read("Kendrick Lamar - Money Trees (feat. Jay Rock) [Official Audio]").guess().unwrap();
    assert_eq!(tags.title, "Money Trees");
    assert!(tags.retitle);
    assert_eq!(tags.artists, s(&["Kendrick Lamar", "Jay Rock"]));
    assert!(tags.album_artists.is_empty() && tags.album.is_none() && tags.date.is_none());
    assert_eq!(read("Money Trees").guess(), None, "no artist, no guess");
}

#[test]
fn cleaning_keeps_the_artist() {
    assert_eq!(clean("01. A - Song (Official Audio)"), "A - Song");
    assert_eq!(clean("Song (feat. B)"), "Song");
}
