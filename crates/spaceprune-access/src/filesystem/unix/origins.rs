//! Native source names for captured entries that survive process exit or restoration conflicts.

use super::*;
use std::{cell::Cell, fs::File, io::Write};

pub(super) const NAME: &str = "original-paths.jsonl";

pub(super) struct Origins {
    file: File,
    end: Cell<u64>,
}

#[derive(serde::Serialize)]
struct Origin<'a> {
    slot: &'a str,
    source: crate::NativePath,
}

impl Origins {
    pub(super) fn create(staging: &OwnedFd) -> io::Result<Self> {
        openat(
            staging,
            NAME,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::APPEND | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
        )
        .map(|fd| Self {
            file: File::from(fd),
            end: Cell::new(0),
        })
        .map_err(Into::into)
    }

    pub(super) fn record(&self, slot: &str, source: &Path) -> io::Result<u64> {
        // No userspace buffer: finish the native-path record before moving its object. A process
        // exit can leave an unused record, but never a captured slot whose origin was only in RAM.
        let start = self.end.get();
        let mut record = serde_json::to_vec(&Origin {
            slot,
            source: crate::NativePath::from_path(source),
        })?;
        record.push(b'\n');
        (&self.file).write_all(&record)?;
        self.end.set(start + record.len() as u64);
        Ok(start)
    }

    // Captures settle in stack order. Remove their records only after all their objects have
    // been deleted or restored; keep the whole branch when any restoration reports survivors.
    pub(super) fn forget_after(&self, start: u64) -> io::Result<()> {
        self.file.set_len(start)?;
        self.end.set(start);
        Ok(())
    }
}

pub(super) fn has_captured_entries(staging: &OwnedFd) -> io::Result<bool> {
    let mut entries = Dir::read_from(staging)?;
    while let Some(entry) = entries.read() {
        let entry = entry?;
        if !matches!(entry.file_name().to_bytes(), b"." | b"..")
            && entry.file_name().to_bytes() != NAME.as_bytes()
        {
            return Ok(true);
        }
    }
    Ok(false)
}
