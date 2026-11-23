//! Rolling files: roll at a size on a record boundary, keep some plain, zip some, delete the
//! oldest (design §9.3).

use std::fs;
use std::io::Read;
use std::path::Path;

use mp_log::{RollConfig, RollingFile};

fn cfg(file_size: u64, plain_files: usize, zip_archives: usize) -> RollConfig {
    RollConfig {
        file_size,
        plain_files,
        zip_archives,
    }
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
}

fn unzip(path: &Path) -> String {
    let mut z = zip::ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
    assert_eq!(z.len(), 1);
    let mut s = String::new();
    z.by_index(0).unwrap().read_to_string(&mut s).unwrap();
    s
}

fn record(n: usize) -> Vec<u8> {
    // 10 bytes each: "rec-0007|\n" style
    format!("rec-{n:04}|\n").into_bytes()
}

#[test]
fn the_default_is_1mb_two_plain_three_zipped() {
    let d = RollConfig::default();
    assert_eq!(
        (d.file_size, d.plain_files, d.zip_archives),
        (1024 * 1024, 2, 3)
    );
}

#[test]
fn records_below_the_size_stay_in_the_live_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut f =
        RollingFile::open(dir.path(), "transcript", "log", cfg(100, 2, 3), Vec::new()).unwrap();
    for n in 0..5 {
        f.write_record(&record(n)).unwrap();
    }
    f.close().unwrap();
    assert_eq!(names(dir.path()), ["transcript.log"]);
    assert_eq!(read(&dir.path().join("transcript.log")).lines().count(), 5);
}

#[test]
fn a_roll_happens_on_a_record_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let mut f = RollingFile::open(dir.path(), "t", "log", cfg(25, 2, 3), Vec::new()).unwrap();
    for n in 0..3 {
        f.write_record(&record(n)).unwrap();
    }
    f.close().unwrap();
    // Two 10-byte records fit in 25; the third would pass it, so it starts a new file.
    assert_eq!(read(&dir.path().join("t.1.log")), "rec-0000|\nrec-0001|\n");
    assert_eq!(read(&dir.path().join("t.log")), "rec-0002|\n");
}

#[test]
fn a_record_bigger_than_the_size_still_goes_in_whole() {
    let dir = tempfile::tempdir().unwrap();
    let mut f = RollingFile::open(dir.path(), "t", "log", cfg(5, 2, 3), Vec::new()).unwrap();
    f.write_record(&record(1)).unwrap();
    f.write_record(&record(2)).unwrap();
    f.close().unwrap();
    assert_eq!(read(&dir.path().join("t.1.log")), "rec-0001|\n");
    assert_eq!(read(&dir.path().join("t.log")), "rec-0002|\n");
}

#[test]
fn older_files_are_zipped_and_the_oldest_deleted() {
    let dir = tempfile::tempdir().unwrap();
    let mut f = RollingFile::open(dir.path(), "t", "log", cfg(10, 2, 3), Vec::new()).unwrap();
    // One record per file: records 0..=7 make eight files, of which five are kept.
    for n in 0..8 {
        f.write_record(&record(n)).unwrap();
    }
    f.close().unwrap();
    assert_eq!(
        names(dir.path()),
        ["t.1.log", "t.2.zip", "t.3.zip", "t.4.zip", "t.log"]
    );
    assert_eq!(read(&dir.path().join("t.log")), "rec-0007|\n");
    assert_eq!(read(&dir.path().join("t.1.log")), "rec-0006|\n");
    assert_eq!(unzip(&dir.path().join("t.2.zip")), "rec-0005|\n");
    assert_eq!(unzip(&dir.path().join("t.3.zip")), "rec-0004|\n");
    assert_eq!(unzip(&dir.path().join("t.4.zip")), "rec-0003|\n");
}

#[test]
fn the_counts_are_settings() {
    let dir = tempfile::tempdir().unwrap();
    let mut f = RollingFile::open(dir.path(), "w", "jsonl", cfg(10, 1, 2), Vec::new()).unwrap();
    for n in 0..6 {
        f.write_record(&record(n)).unwrap();
    }
    f.close().unwrap();
    assert_eq!(names(dir.path()), ["w.1.zip", "w.2.zip", "w.jsonl"]);
    assert_eq!(unzip(&dir.path().join("w.1.zip")), "rec-0004|\n");
    assert_eq!(unzip(&dir.path().join("w.2.zip")), "rec-0003|\n");
}

#[test]
fn every_new_file_starts_with_the_header() {
    let dir = tempfile::tempdir().unwrap();
    let header = b"HEADER\n".to_vec();
    let mut f = RollingFile::open(dir.path(), "t", "log", cfg(20, 2, 3), header).unwrap();
    for n in 0..2 {
        f.write_record(&record(n)).unwrap();
    }
    f.close().unwrap();
    assert_eq!(read(&dir.path().join("t.1.log")), "HEADER\nrec-0000|\n");
    assert_eq!(read(&dir.path().join("t.log")), "HEADER\nrec-0001|\n");
}

#[test]
fn reopening_appends_and_counts_what_is_there() {
    let dir = tempfile::tempdir().unwrap();
    let mut f = RollingFile::open(dir.path(), "t", "log", cfg(25, 2, 3), Vec::new()).unwrap();
    f.write_record(&record(0)).unwrap();
    f.close().unwrap();
    let mut f = RollingFile::open(dir.path(), "t", "log", cfg(25, 2, 3), Vec::new()).unwrap();
    f.write_record(&record(1)).unwrap();
    f.write_record(&record(2)).unwrap();
    f.close().unwrap();
    assert_eq!(read(&dir.path().join("t.1.log")), "rec-0000|\nrec-0001|\n");
    assert_eq!(read(&dir.path().join("t.log")), "rec-0002|\n");
}
