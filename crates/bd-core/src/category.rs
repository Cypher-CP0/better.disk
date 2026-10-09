//! File type classification by extension (FR07).
//!
//! The mapping is versioned so saved snapshots record which mapping
//! classified them. Type is one filter among several: location-based
//! categories (Downloads, Apps, System) are separate and can overlap with a
//! type, so category totals must never be added together as a drive total.

use std::ffi::OsStr;
use std::path::Path;

/// Bump when the extension lists change.
pub const FILE_TYPE_MAPPING_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileType {
    Documents,
    Videos,
    Archives,
    Music,
    Images,
    Other,
}

impl FileType {
    pub const ALL: [FileType; 6] = [
        FileType::Documents,
        FileType::Videos,
        FileType::Archives,
        FileType::Music,
        FileType::Images,
        FileType::Other,
    ];
}

const DOCUMENTS: &[&str] = &[
    "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods", "odp", "rtf", "txt", "md",
    "csv", "epub", "tex", "xps", "one", "pages", "numbers", "key",
];
const VIDEOS: &[&str] = &[
    "mp4", "mkv", "mov", "avi", "wmv", "webm", "m4v", "mpg", "mpeg", "flv", "3gp", "m2ts", "mts",
    "vob",
];
const ARCHIVES: &[&str] = &[
    "zip", "rar", "7z", "tar", "gz", "tgz", "bz2", "xz", "zst", "lz", "lzma", "cab", "iso", "img",
    "vhd", "vhdx", "wim", "dmg",
];
const MUSIC: &[&str] = &[
    "mp3", "flac", "wav", "aac", "m4a", "ogg", "opus", "wma", "aiff", "aif", "mid", "midi",
];
const IMAGES: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "bmp", "tif", "tiff", "webp", "heic", "heif", "avif", "svg",
    "ico", "psd", "raw", "cr2", "cr3", "nef", "arw", "dng",
];

/// Classify a file name. Names without an extension (including dot-files
/// such as `.gitignore`) are `Other`.
pub fn classify(name: &OsStr) -> FileType {
    let Some(ext) = Path::new(name).extension().and_then(OsStr::to_str) else {
        return FileType::Other;
    };
    let ext = ext.to_ascii_lowercase();
    let ext = ext.as_str();
    if DOCUMENTS.contains(&ext) {
        FileType::Documents
    } else if VIDEOS.contains(&ext) {
        FileType::Videos
    } else if ARCHIVES.contains(&ext) {
        FileType::Archives
    } else if MUSIC.contains(&ext) {
        FileType::Music
    } else if IMAGES.contains(&ext) {
        FileType::Images
    } else {
        FileType::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_case_insensitively() {
        assert_eq!(classify(OsStr::new("Holiday.MP4")), FileType::Videos);
        assert_eq!(classify(OsStr::new("report.Pdf")), FileType::Documents);
        assert_eq!(classify(OsStr::new("backup.tar.gz")), FileType::Archives);
        assert_eq!(classify(OsStr::new("song.flac")), FileType::Music);
        assert_eq!(classify(OsStr::new("IMG_0001.HEIC")), FileType::Images);
    }

    #[test]
    fn no_extension_is_other() {
        assert_eq!(classify(OsStr::new(".gitignore")), FileType::Other);
        assert_eq!(classify(OsStr::new("Makefile")), FileType::Other);
        assert_eq!(classify(OsStr::new("setup.exe")), FileType::Other);
    }

    #[test]
    fn lists_do_not_overlap() {
        let lists = [DOCUMENTS, VIDEOS, ARCHIVES, MUSIC, IMAGES];
        for (i, a) in lists.iter().enumerate() {
            for b in lists.iter().skip(i + 1) {
                for ext in a.iter() {
                    assert!(!b.contains(ext), "{ext} appears in two lists");
                }
            }
        }
    }
}
