//! Chart history (a time-bounded ring buffer) and the CSV recorder thread.

use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::{unbounded, RecvTimeoutError, Sender};
use dps150_device::{Mode, ProtState, Sample};

/// Samples of the last `max_secs` seconds.
pub struct History {
    samples: VecDeque<Sample>,
    max_secs: f64,
}

impl History {
    /// History that keeps `max_secs` seconds.
    pub fn new(max_secs: f64) -> Self {
        Self {
            samples: VecDeque::new(),
            max_secs,
        }
    }

    /// Add a sample and drop the ones that are too old.
    pub fn push(&mut self, s: Sample) {
        self.samples.push_back(s);
        while let Some(first) = self.samples.front() {
            if s.t - first.t > self.max_secs {
                self.samples.pop_front();
            } else {
                break;
            }
        }
    }

    /// All samples, oldest first.
    pub fn iter(&self) -> impl Iterator<Item = &Sample> {
        self.samples.iter()
    }

    /// Newest sample.
    pub fn last(&self) -> Option<&Sample> {
        self.samples.back()
    }

    /// Number of samples kept.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// True when empty.
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Remove everything.
    pub fn clear(&mut self) {
        self.samples.clear();
    }
}

/// One row of a recording.
#[derive(Debug, Clone, Copy)]
pub struct Row {
    /// The measurement.
    pub sample: Sample,
    /// Output enabled at that moment.
    pub output: bool,
    /// Regulation mode.
    pub mode: Mode,
    /// Protection state.
    pub prot: ProtState,
}

enum Msg {
    Row(Row),
    Stop,
}

/// A running CSV recording.
pub struct Recorder {
    tx: Sender<Msg>,
    thread: Option<JoinHandle<std::io::Result<u64>>>,
    path: PathBuf,
    rows: u64,
}

impl Recorder {
    /// Create `dir/YYYY-MM-DD_HHMMSS.csv` and start the writer thread.
    pub fn start(dir: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let name = chrono::Local::now()
            .format("%Y-%m-%d_%H%M%S.csv")
            .to_string();
        let path = dir.join(name);
        let file = File::create(&path)?;
        let (tx, rx) = unbounded::<Msg>();
        let thread = std::thread::Builder::new()
            .name("csv-recorder".into())
            .spawn(move || {
                let mut w = BufWriter::new(file);
                writeln!(w, "time,unix_s,volts,amps,watts,output,mode,protection")?;
                let mut n = 0u64;
                let mut last_flush = Instant::now();
                loop {
                    match rx.recv_timeout(Duration::from_millis(500)) {
                        Ok(Msg::Row(r)) => {
                            let time = chrono::DateTime::from_timestamp_millis(
                                (r.sample.t * 1000.0) as i64,
                            )
                            .map(|d| {
                                d.with_timezone(&chrono::Local)
                                    .format("%Y-%m-%dT%H:%M:%S%.3f")
                                    .to_string()
                            })
                            .unwrap_or_default();
                            writeln!(
                                w,
                                "{time},{:.3},{:.4},{:.4},{:.4},{},{},{}",
                                r.sample.t,
                                r.sample.volts,
                                r.sample.amps,
                                r.sample.watts,
                                r.output as u8,
                                r.mode.label(),
                                r.prot.label()
                            )?;
                            n += 1;
                        }
                        Ok(Msg::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                        Err(RecvTimeoutError::Timeout) => {}
                    }
                    if last_flush.elapsed() >= Duration::from_secs(1) {
                        w.flush()?;
                        last_flush = Instant::now();
                    }
                }
                w.flush()?;
                Ok(n)
            })?;
        Ok(Self {
            tx,
            thread: Some(thread),
            path,
            rows: 0,
        })
    }

    /// Queue a row.
    pub fn write(&mut self, row: Row) {
        if self.tx.send(Msg::Row(row)).is_ok() {
            self.rows += 1;
        }
    }

    /// File being written.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Rows queued so far.
    pub fn rows(&self) -> u64 {
        self.rows
    }

    /// Stop and wait for the file to be flushed. Returns the rows written.
    pub fn stop(mut self) -> std::io::Result<u64> {
        self.finish()
    }

    fn finish(&mut self) -> std::io::Result<u64> {
        let _ = self.tx.send(Msg::Stop);
        match self.thread.take() {
            Some(t) => t
                .join()
                .unwrap_or_else(|_| Err(std::io::Error::other("recorder panicked"))),
            None => Ok(0),
        }
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(t: f64) -> Sample {
        Sample {
            t,
            volts: 1.0,
            amps: 0.1,
            watts: 0.1,
        }
    }

    #[test]
    fn history_is_bounded_by_time() {
        let mut h = History::new(10.0);
        for i in 0..100 {
            h.push(s(i as f64));
        }
        assert_eq!(h.len(), 11);
        assert_eq!(h.iter().next().unwrap().t, 89.0);
    }

    #[test]
    fn recorder_writes_csv() {
        let dir = std::env::temp_dir().join(format!("dps150-rec-{}", std::process::id()));
        let mut r = Recorder::start(&dir).unwrap();
        let path = r.path().to_path_buf();
        for i in 0..3 {
            r.write(Row {
                sample: s(1_700_000_000.0 + i as f64),
                output: true,
                mode: Mode::Cv,
                prot: ProtState::Normal,
            });
        }
        assert_eq!(r.stop().unwrap(), 3);
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 4);
        assert!(lines[1].ends_with(",1,CV,Normal"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
