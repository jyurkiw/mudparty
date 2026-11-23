//! One stream's files (design §9.3).
//!
//! For a stream `transcript.log` with two plain files and three archives:
//!
//! ```text
//! transcript.log     the live file
//! transcript.1.log   the one before it
//! transcript.2.zip   older, zipped
//! transcript.3.zip
//! transcript.4.zip   the oldest, deleted when the next one is archived
//! ```

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

/// How a stream rolls. The defaults are the design's: 1 MB files, two plain, three zipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RollConfig {
    /// Roll before a record would take the live file past this many bytes.
    pub file_size: u64,
    /// Plain files kept, the live one included. At least 1.
    pub plain_files: usize,
    /// Zipped files kept after those.
    pub zip_archives: usize,
}

impl Default for RollConfig {
    fn default() -> Self {
        Self {
            file_size: 1024 * 1024,
            plain_files: 2,
            zip_archives: 3,
        }
    }
}

/// One stream's live file and its older files.
#[derive(Debug)]
pub struct RollingFile {
    dir: PathBuf,
    stem: String,
    ext: String,
    cfg: RollConfig,
    /// Written at the start of every new file.
    header: Vec<u8>,
    file: File,
    size: u64,
    zipping: Option<JoinHandle<io::Result<()>>>,
}

impl RollingFile {
    /// Opens `<dir>/<stem>.<ext>`, appending to it when it exists. `header` starts every
    /// file this stream begins.
    pub fn open(
        dir: &Path,
        stem: &str,
        ext: &str,
        cfg: RollConfig,
        header: Vec<u8>,
    ) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let cfg = RollConfig {
            plain_files: cfg.plain_files.max(1),
            ..cfg
        };
        let live = dir.join(format!("{stem}.{ext}"));
        let (file, size) = open_live(&live, &header)?;
        Ok(Self {
            dir: dir.to_owned(),
            stem: stem.to_owned(),
            ext: ext.to_owned(),
            cfg,
            header,
            file,
            size,
            zipping: None,
        })
    }

    /// Writes one whole record, first rolling if it would take the live file past the size.
    /// A record bigger than the size still goes in whole, in a file of its own.
    pub fn write_record(&mut self, record: &[u8]) -> io::Result<()> {
        let fresh = self.size <= self.header.len() as u64;
        if !fresh && self.size + record.len() as u64 > self.cfg.file_size {
            self.roll()?;
        }
        self.file.write_all(record)?;
        self.size += record.len() as u64;
        Ok(())
    }

    /// Flushes and waits for any zipping still running.
    pub fn close(mut self) -> io::Result<()> {
        self.file.flush()?;
        self.wait_for_zip()
    }

    fn live(&self) -> PathBuf {
        self.dir.join(format!("{}.{}", self.stem, self.ext))
    }

    fn plain(&self, k: usize) -> PathBuf {
        self.dir.join(format!("{}.{k}.{}", self.stem, self.ext))
    }

    fn zipping_path(&self) -> PathBuf {
        self.dir.join(format!("{}.zipping.{}", self.stem, self.ext))
    }

    fn wait_for_zip(&mut self) -> io::Result<()> {
        match self.zipping.take() {
            Some(job) => job
                .join()
                .unwrap_or_else(|_| Err(io::Error::other("the zip thread panicked"))),
            None => Ok(()),
        }
    }

    fn roll(&mut self) -> io::Result<()> {
        self.wait_for_zip()?;
        self.file.flush()?;
        let p = self.cfg.plain_files;
        // The file leaving the plain set: the oldest plain one, or the live one when only one
        // is kept. It is renamed aside first so the shift below can reuse its name.
        let leaving = if p == 1 {
            self.live()
        } else {
            self.plain(p - 1)
        };
        let zipping = self.zipping_path();
        let archive = leaving.exists();
        if archive {
            fs::rename(&leaving, &zipping)?;
        }
        if p > 1 {
            for k in (1..p - 1).rev() {
                if self.plain(k).exists() {
                    fs::rename(self.plain(k), self.plain(k + 1))?;
                }
            }
            fs::rename(self.live(), self.plain(1))?;
        }
        (self.file, self.size) = open_live(&self.live(), &self.header)?;
        if archive {
            let job = ZipJob {
                dir: self.dir.clone(),
                stem: self.stem.clone(),
                entry: format!("{}.{}", self.stem, self.ext),
                source: zipping,
                first: p,
                count: self.cfg.zip_archives,
            };
            self.zipping = Some(std::thread::spawn(move || job.run()));
        }
        Ok(())
    }
}

impl Drop for RollingFile {
    fn drop(&mut self) {
        // close() is the way to see an error; a drop still lets the zip finish.
        let _ = self.file.flush();
        let _ = self.wait_for_zip();
    }
}

/// Opens the live file for appending, writing `header` first when the file is new. Returns
/// the file and its size.
fn open_live(path: &Path, header: &[u8]) -> io::Result<(File, u64)> {
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let mut size = file.metadata()?.len();
    if size == 0 && !header.is_empty() {
        file.write_all(header)?;
        size = header.len() as u64;
    }
    Ok((file, size))
}

/// Shifts the archives up one and zips `source` in as the newest.
struct ZipJob {
    dir: PathBuf,
    stem: String,
    entry: String,
    source: PathBuf,
    /// The number the newest archive gets.
    first: usize,
    count: usize,
}

impl ZipJob {
    fn zip(&self, k: usize) -> PathBuf {
        self.dir.join(format!("{}.{k}.zip", self.stem))
    }

    fn run(self) -> io::Result<()> {
        if self.count == 0 {
            return fs::remove_file(&self.source);
        }
        let last = self.first + self.count - 1;
        if self.zip(last).exists() {
            fs::remove_file(self.zip(last))?;
        }
        for k in (self.first..last).rev() {
            if self.zip(k).exists() {
                fs::rename(self.zip(k), self.zip(k + 1))?;
            }
        }
        let out = File::create(self.zip(self.first))?;
        let mut w = zip::ZipWriter::new(out);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        w.start_file(self.entry.as_str(), opts)
            .map_err(io::Error::other)?;
        io::copy(&mut File::open(&self.source)?, &mut w)?;
        w.finish().map_err(io::Error::other)?;
        fs::remove_file(&self.source)
    }
}
