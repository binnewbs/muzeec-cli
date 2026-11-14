//! Library scanning, tag extraction, artwork, and file watching.

pub mod art;
pub mod scan;
pub mod tags;
pub mod watch;

pub use art::ArtCache;
pub use scan::{ScanEvent, ScanOptions, ScanReport, Scanner};
pub use tags::read_tags;
