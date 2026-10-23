//! Stage 7 on one folder: what its listing says has gone from what the index holds in it, and
//! marking that missing (`design/scanning.md` §8).

use std::borrow::Cow;
use std::collections::HashSet;
use std::path::PathBuf;

use super::writer::{Halt, Writer};
use super::{Scan, ScanContext};
use crate::discover::Directory;
use crate::store::IndexedFolder;
use crate::track::TrackId;

/// What `dir`'s listing no longer shows of what the index holds in it, once each of its files
/// has taken its own track from `indexed` ([`IndexedFolder::take`]): the tracks left that are not
/// missing yet, and the path of each indexed folder it does not list. A name the walk could not
/// read may still be there, so neither includes one, and both are empty when the listing itself
/// failed partway, which is reported on `dir`, since then any name may be absent from it.
pub(super) fn unlisted(dir: &Directory, indexed: IndexedFolder) -> (Vec<TrackId>, Vec<PathBuf>) {
    if dir.unreadable.iter().any(|entry| entry.path == dir.path) {
        return (Vec::new(), Vec::new());
    }
    let unread: HashSet<Cow<str>> = dir
        .unreadable
        .iter()
        .filter_map(|entry| entry.path.file_name())
        .map(|name| name.to_string_lossy())
        .collect();
    let files = indexed
        .files
        .into_iter()
        .filter(|(name, file)| !file.missing && !unread.contains(name.as_str()));
    let listed: HashSet<Cow<str>> = dir.folders.iter().map(|name| name.to_string_lossy()).collect();
    let folders = indexed
        .folders
        .into_iter()
        .filter(|name| !listed.contains(name.as_str()) && !unread.contains(name.as_str()));
    (
        files.map(|(_, file)| file.track_id).collect(),
        folders.map(|name| dir.path.join(name)).collect(),
    )
}

/// Puts `missing` in the writer's batch, and every track not missing yet under each folder of
/// `vanished`, at any depth, read from the index a folder at a time ([`crate::Store::indexed_in`]),
/// one track at a time with a checkpoint before each. So marks fill batches as any other outcome
/// does, however much a folder held (`design/scanning.md` §7). Fails as the checkpoint does, or
/// with [`Halt::Ended`] once a store failure has ended the scan.
pub(super) fn mark(
    ctx: &ScanContext<'_>,
    scan: &mut Scan,
    writer: &mut Writer,
    missing: Vec<TrackId>,
    mut vanished: Vec<PathBuf>,
) -> Result<(), Halt> {
    for id in missing {
        writer.checkpoint(ctx, scan)?;
        writer.batch.missing.push(id);
    }
    while let Some(folder) = vanished.pop() {
        let indexed = ctx.store.indexed_in(ctx.library.id, &folder);
        let indexed = indexed.map_err(|e| writer.fail(ctx, scan, &e))?;
        vanished.extend(indexed.folders.iter().map(|name| folder.join(name)));
        for file in indexed.files.into_values().filter(|file| !file.missing) {
            writer.checkpoint(ctx, scan)?;
            writer.batch.missing.push(file.track_id);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet, HashMap};
    use std::io;

    use super::*;
    use crate::discover::WalkError;
    use crate::store::IndexedFile;

    /// A listing of `/m/A` that names nothing, with `unreadable` entries.
    fn empty_listing(unreadable: Vec<WalkError>) -> Directory {
        Directory {
            path: PathBuf::from("/m/A"),
            listed: true,
            candidates: Vec::new(),
            files: BTreeMap::new(),
            folders: BTreeSet::new(),
            skipped_links: Vec::new(),
            unreadable,
        }
    }

    /// What the index holds in `/m/A`: track 7 at `a.flac`, and the folder `Sub`.
    fn indexed() -> IndexedFolder {
        let file = IndexedFile {
            track_id: 7,
            size: 1,
            mtime_ms: Some(1),
            missing: false,
            sidecar_stamp: 0,
        };
        IndexedFolder {
            files: HashMap::from([("a.flac".into(), file)]),
            folders: vec!["Sub".into()],
        }
    }

    /// An error reading an entry of the listing.
    fn error(path: &str) -> WalkError {
        WalkError { path: PathBuf::from(path), error: io::Error::other("I/O error") }
    }

    /// A listing that names neither the file nor the folder the index holds marks both, unless it
    /// failed partway, which is reported on the folder itself, or could not read either entry.
    #[test]
    fn only_a_whole_listing_marks_what_it_lacks() {
        let marked = (vec![7], vec![PathBuf::from("/m/A/Sub")]);
        assert_eq!(unlisted(&empty_listing(Vec::new()), indexed()), marked);
        let partway = empty_listing(vec![error("/m/A")]);
        assert_eq!(unlisted(&partway, indexed()), (vec![], vec![]));
        let unread = empty_listing(vec![error("/m/A/a.flac"), error("/m/A/Sub")]);
        assert_eq!(unlisted(&unread, indexed()), (vec![], vec![]));
    }
}
