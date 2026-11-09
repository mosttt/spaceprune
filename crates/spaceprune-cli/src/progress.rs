//! Live, password-safe progress on stderr without taking over terminal input.

use std::{
    collections::{BTreeMap, VecDeque},
    io::{self, IsTerminal, Write},
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crossterm::terminal::size;

mod screen;
use screen::ProgressScreen;
use spaceprune::api::{
    CancellationToken, EventBus, EventListenerId, ProgressEvent, ProgressPhase, ProgressSink,
    SpacePruneEvent,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    output::receipt::{Receipt, field_label},
    theme::{Theme, Tone},
};

#[derive(Clone, Copy)]
enum Operation {
    Scan,
    Clean,
}

struct Activity {
    operation: Operation,
    started: Instant,
    updated: Instant,
    module_started: Instant,
    active_module: Option<String>,
    module_names: BTreeMap<String, String>,
    history: VecDeque<(Duration, ProgressEvent)>,
    event: Option<ProgressEvent>,
    checks: usize,
    total_checks: Option<usize>,
    items: usize,
    succeeded: usize,
    failures: usize,
    completed_items: BTreeMap<(String, String), usize>,
    measurements: BTreeMap<(String, String), spaceprune::api::ScanMeasurement>,
    cleanup_sizes: BTreeMap<(String, String), (u64, usize)>,
    memory_sizes: BTreeMap<(String, String), (u64, usize)>,
}

impl Activity {
    fn new(operation: Operation, items: usize) -> Self {
        let started = Instant::now();
        Self {
            operation,
            started,
            updated: started,
            module_started: started,
            active_module: None,
            module_names: BTreeMap::new(),
            history: VecDeque::new(),
            event: None,
            checks: 0,
            total_checks: None,
            items,
            succeeded: 0,
            failures: 0,
            completed_items: BTreeMap::new(),
            measurements: BTreeMap::new(),
            cleanup_sizes: BTreeMap::new(),
            memory_sizes: BTreeMap::new(),
        }
    }

    fn record(&mut self, mut event: ProgressEvent) {
        event.message = display_message(&event.message);
        let now = Instant::now();
        if event.module_id.is_some() && self.active_module != event.module_id {
            self.module_started = now;
            self.active_module = event.module_id.clone();
        }
        if self.history.back().is_none_or(|(_, previous)| {
            previous.message != event.message || previous.phase != event.phase
        }) {
            self.history
                .push_back((now.duration_since(self.started), event.clone()));
            if self.history.len() > 64 {
                self.history.pop_front();
            }
        }
        self.event = Some(event);
        self.updated = now;
    }

    fn history_lines(&self, rows: usize) -> Vec<(String, Tone)> {
        self.history
            .iter()
            .enumerate()
            .skip(self.history.len().saturating_sub(rows))
            .map(|(index, (elapsed, event))| {
                let active =
                    index + 1 == self.history.len() && event.phase != ProgressPhase::Complete;
                let marker = if active { '›' } else { ' ' };
                (
                    format!("{marker} {}  {}", duration(*elapsed), event.message),
                    if active {
                        if event.phase == ProgressPhase::Authorize {
                            Tone::Warning
                        } else {
                            Tone::Accent
                        }
                    } else {
                        Tone::Muted
                    },
                )
            })
            .collect()
    }

    fn lines(&self, now: Instant, cancelled: bool, width: usize) -> Vec<(String, Tone)> {
        let elapsed = now.duration_since(self.started);
        let step_elapsed = now.duration_since(self.updated);
        let spinner = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        let frame = (elapsed.as_millis() / 100 % spinner.len() as u128) as usize;
        let title = match self.operation {
            Operation::Scan => crate::i18n::text("terminal.scanning"),
            Operation::Clean => crate::i18n::text("terminal.cleaning"),
        };
        let phase =
            self.event.as_ref().map_or(
                crate::i18n::text("terminal.preparing"),
                |event| match event.phase {
                    ProgressPhase::Load => crate::i18n::text("terminal.loading"),
                    ProgressPhase::Scan => crate::i18n::text("terminal.checking"),
                    ProgressPhase::Measure => crate::i18n::text("terminal.measuring"),
                    ProgressPhase::Plan => crate::i18n::text("terminal.planning"),
                    ProgressPhase::Authorize => crate::i18n::text("terminal.authorizing"),
                    ProgressPhase::Clean => crate::i18n::text("terminal.executing"),
                    ProgressPhase::Complete => crate::i18n::text("terminal.finishing"),
                },
            );
        let details = self.event.as_ref().map_or(phase.to_owned(), |event| {
            format!("{phase} · {}", event.message)
        });
        let footer = if cancelled {
            crate::i18n::text("terminal.stop_requested").to_owned()
        } else if step_elapsed >= Duration::from_secs(3) {
            crate::i18n::message(
                "terminal.still_working",
                &[("p0", (duration(step_elapsed)).to_string())],
            )
        } else {
            crate::i18n::text("terminal.ctrlc_stops").to_owned()
        };
        vec![
            (
                format!("{} {title} · {}", spinner[frame], duration(elapsed)),
                if cancelled {
                    Tone::Warning
                } else {
                    Tone::Accent
                },
            ),
            (
                self.progress_bar(elapsed, width),
                if cancelled {
                    Tone::Warning
                } else {
                    Tone::Success
                },
            ),
            (self.module_progress_bar(now, width), Tone::Secondary),
            (format!("  {details}"), Tone::Secondary),
            (self.size_summary(), Tone::Accent),
            (
                format!("  {footer}"),
                if cancelled {
                    Tone::Warning
                } else {
                    Tone::Muted
                },
            ),
        ]
    }

    fn frame_lines(
        &self,
        now: Instant,
        cancelled: bool,
        width: usize,
        height: u16,
    ) -> Vec<(String, Tone)> {
        let mut lines = self.lines(now, cancelled, width);
        let history_rows = usize::from(height.saturating_sub(8)).min(5);
        if history_rows > 0 {
            let mut history = vec![(
                crate::i18n::text("terminal.recent_activity").to_owned(),
                Tone::Secondary,
            )];
            history.extend(self.history_lines(history_rows));
            while history.len() <= history_rows {
                history.push((String::new(), Tone::Muted));
            }
            lines.splice(5..5, history);
        }
        if height < 7 {
            lines.remove(4);
        }
        if height < 6 {
            lines.remove(2);
        }
        if height < 5 {
            lines.remove(1);
        }
        if height < 4 {
            lines.remove(1);
        }
        if height < 3 {
            lines.truncate(1);
        }
        lines
    }

    fn progress_bar(&self, elapsed: Duration, width: usize) -> String {
        let step = self.event.as_ref().and_then(|event| {
            event
                .current
                .zip(event.total)
                .filter(|(_, total)| *total > 0)
        });
        let (label, fraction) = match self.operation {
            Operation::Clean if self.items > 0 => (
                crate::i18n::text("terminal.total_items"),
                Some((
                    self.completed_items.values().sum::<usize>().min(self.items),
                    self.items,
                )),
            ),
            Operation::Scan if self.total_checks.is_some_and(|total| total > 0) => (
                crate::i18n::text("terminal.total_checks"),
                Some((self.checks, self.total_checks.unwrap())),
            ),
            _ => (crate::i18n::text("terminal.current_step"), step),
        };
        let summary = match self.operation {
            Operation::Scan => {
                crate::i18n::message("terminal.items_found", &[("p0", (self.items).to_string())])
            }
            Operation::Clean if self.failures > 0 => crate::i18n::message(
                "terminal.progress_failures",
                &[("p0", (self.failures).to_string())],
            ),
            Operation::Clean => String::new(),
        };
        format!(
            "{}{summary}",
            render_bar(
                label,
                fraction,
                elapsed,
                width.saturating_sub(summary.width())
            )
        )
    }

    fn size_summary(&self) -> String {
        use spaceprune::api::fs::format_bytes;
        match self.operation {
            Operation::Scan if !self.measurements.is_empty() => {
                let mut total = spaceprune::api::ScanMeasurement::default();
                for measurement in self.measurements.values() {
                    total.merge(*measurement);
                }
                crate::i18n::message(
                    "terminal.live_scan_size",
                    &[
                        ("count", total.checked_items.to_string()),
                        ("size", format_bytes(total.logical_bytes)),
                        ("recommended", format_bytes(total.recommended_bytes)),
                        ("unknown", total.unknown_size_items.to_string()),
                    ],
                )
            }
            Operation::Clean if !self.cleanup_sizes.is_empty() || !self.memory_sizes.is_empty() => {
                let (bytes, unknown) = self
                    .cleanup_sizes
                    .values()
                    .fold((0u64, 0usize), |(b, u), (next, missing)| {
                        (b.saturating_add(*next), u.saturating_add(*missing))
                    });
                let disk = crate::i18n::message(
                    "terminal.live_cleanup_size",
                    &[
                        ("size", format_bytes(bytes)),
                        ("unknown", unknown.to_string()),
                    ],
                );
                if self.memory_sizes.is_empty() {
                    disk
                } else {
                    let (ram, missing) = self.memory_sizes.values().fold(
                        (0u64, 0usize),
                        |(b, u), (next, unknown)| {
                            (b.saturating_add(*next), u.saturating_add(*unknown))
                        },
                    );
                    let memory = format!(
                        "{} {} · {} {}",
                        crate::i18n::text("terminal.memory_reduction"),
                        format_bytes(ram),
                        missing,
                        crate::i18n::text("terminal.unknown_size")
                    );
                    if self.cleanup_sizes.is_empty() {
                        memory
                    } else {
                        format!("{disk} · {memory}")
                    }
                }
            }
            _ => String::new(),
        }
    }

    fn module_progress_bar(&self, now: Instant, width: usize) -> String {
        let module = self
            .active_module
            .as_deref()
            .map(|id| self.module_names.get(id).map_or(id, String::as_str))
            .unwrap_or(crate::i18n::text("terminal.preparing_module"));
        let fraction = self
            .event
            .as_ref()
            .and_then(|event| event.current.zip(event.total))
            .filter(|(_, total)| *total > 0);
        let bar = render_bar(
            &crate::i18n::message(
                "terminal.module_progress",
                &[("module", (module).to_string())],
            ),
            fraction,
            now.duration_since(self.module_started),
            width.saturating_sub(16),
        );
        crate::i18n::message(
            "terminal.elapsed",
            &[
                ("bar", (bar).to_string()),
                (
                    "p0",
                    (duration(now.duration_since(self.module_started))).to_string(),
                ),
            ],
        )
    }
}

fn render_bar(
    label: &str,
    fraction: Option<(usize, usize)>,
    elapsed: Duration,
    width: usize,
) -> String {
    let description = if let Some((current, total)) = fraction {
        let current = current.min(total);
        format!(
            "{label} {current}/{total} · {}%",
            current as u128 * 100 / total as u128
        )
    } else {
        crate::i18n::message("terminal.unknown_total", &[("label", label.to_owned())])
    };
    // Reserve the real label width so localized copy cannot push the elapsed time offscreen.
    let description = clip(&description, width.saturating_sub(11));
    let columns = width.saturating_sub(description.width() + 5).clamp(6, 36);
    let bar = if let Some((current, total)) = fraction {
        let filled = (current.min(total) as u128 * columns as u128 / total as u128) as usize;
        format!("{}{}", "█".repeat(filled), "░".repeat(columns - filled))
    } else {
        let span = 3.min(columns);
        let position = (elapsed.as_millis() / 100 % (columns - span + 1) as u128) as usize;
        format!(
            "{}{}{}",
            "░".repeat(position),
            "█".repeat(span),
            "░".repeat(columns - position - span)
        )
    };
    format!("  [{bar}] {description}")
}

#[derive(Default)]
struct State {
    activity: Option<Activity>,
    module_names: BTreeMap<String, String>,
    input_depth: usize,
    screen: ProgressScreen,
    shutdown: bool,
    cleanup_elapsed: Option<Duration>,
}

impl State {
    fn draw(&mut self, theme: Theme, cancelled: bool) -> io::Result<()> {
        let Some(activity) = &self.activity else {
            return Ok(());
        };
        let (columns, rows) = size().unwrap_or((80, 24));
        let lines = activity.frame_lines(
            Instant::now(),
            cancelled,
            usize::from(columns.saturating_sub(1)),
            rows,
        );
        self.screen.draw(&lines, theme, columns, rows)
    }

    fn finish(&mut self, live: bool) -> Option<Duration> {
        let activity = self.activity.take()?;
        if live {
            let _ = self.screen.restore();
        }
        Some(activity.started.elapsed())
    }

    fn on_event(&mut self, event: &SpacePruneEvent, theme: Theme, live: bool) {
        match event {
            SpacePruneEvent::ScanStarted { task_count, .. } => {
                let mut activity = Activity::new(Operation::Scan, 0);
                activity.module_names.clone_from(&self.module_names);
                activity.total_checks = *task_count;
                self.activity = Some(activity);
            }
            SpacePruneEvent::TaskScanFinished { item_count, .. } => {
                if let Some(activity) = &mut self.activity {
                    activity.checks += 1;
                    activity.items += item_count;
                }
            }
            SpacePruneEvent::ScanFinished {
                task_count,
                item_count,
                issue_count,
                error_count,
                cancelled,
                stop_reason,
                ..
            } => {
                if let Some(elapsed) = self.finish(live) {
                    let timed_out = *stop_reason
                        == Some(spaceprune::api::OperationStopReason::DeadlineExceeded);
                    let (label, tone) = if timed_out {
                        (crate::i18n::text("terminal.scan_timeout"), Tone::Error)
                    } else if *cancelled {
                        (crate::i18n::text("terminal.scan_stopped"), Tone::Warning)
                    } else if *error_count > 0 {
                        (crate::i18n::text("terminal.scan_errors"), Tone::Error)
                    } else if *issue_count > 0 {
                        (crate::i18n::text("terminal.scan_issues"), Tone::Warning)
                    } else {
                        (crate::i18n::text("terminal.scan_finished"), Tone::Success)
                    };
                    let receipt = Receipt::stderr(theme);
                    let mut output = io::stderr().lock();
                    let _ = write!(output, "{}", receipt.heading(label, tone, Some(elapsed)));
                    let counts = crate::i18n::message(
                        "terminal.checked_counts",
                        &[
                            ("task_count", task_count.to_string()),
                            (
                                "p0",
                                crate::i18n::text(if *task_count == 1 {
                                    "terminal.check"
                                } else {
                                    "terminal.checks"
                                })
                                .to_owned(),
                            ),
                            ("item_count", item_count.to_string()),
                            (
                                "p1",
                                crate::i18n::text(if *item_count == 1 {
                                    "terminal.item"
                                } else {
                                    "terminal.items"
                                })
                                .to_owned(),
                            ),
                        ],
                    );
                    let _ = writeln!(
                        output,
                        "{}",
                        receipt.row(
                            crate::i18n::text("terminal.checked"),
                            &counts,
                            Tone::Secondary
                        )
                    );
                    let issues = format!(
                        "{issue_count} {}",
                        crate::i18n::text(if *issue_count == 1 {
                            "terminal.issue"
                        } else {
                            "terminal.issues"
                        })
                    );
                    let errors = format!(
                        "{error_count} {}",
                        crate::i18n::text(if *error_count == 1 {
                            "terminal.error"
                        } else {
                            "terminal.errors"
                        })
                    );
                    let _ = writeln!(
                        output,
                        "{}{} · {}\n",
                        field_label(crate::i18n::text("terminal.diagnostics")),
                        theme.stderr(
                            issues,
                            if *issue_count > 0 {
                                Tone::Warning
                            } else {
                                Tone::Muted
                            }
                        ),
                        theme.stderr(
                            errors,
                            if *error_count > 0 {
                                Tone::Error
                            } else {
                                Tone::Muted
                            }
                        ),
                    );
                    let _ = output.flush();
                }
            }
            SpacePruneEvent::CleanupStarted { item_count, .. } => {
                self.cleanup_elapsed = None;
                let mut activity = Activity::new(Operation::Clean, *item_count);
                activity.module_names.clone_from(&self.module_names);
                self.activity = Some(activity);
            }
            SpacePruneEvent::CleanupGroupFinished {
                module_id,
                task_id,
                attempted,
                succeeded,
                failure_count,
                error,
                ..
            } => {
                if let Some(activity) = &mut self.activity {
                    activity.checks += 1;
                    activity.succeeded += succeeded;
                    activity.failures += (*failure_count).max(usize::from(error.is_some()));
                    activity
                        .completed_items
                        .insert((module_id.clone(), task_id.clone()), *attempted);
                }
            }
            SpacePruneEvent::CleanupFinished { .. } => {
                // The final report owns the sole cleanup receipt, including service failures.
                self.cleanup_elapsed = self.finish(live);
            }
            _ => {}
        }
    }
}

/// Render engine activity only while scanning or executing; leave reports and input alone.
pub(crate) struct TextProgress {
    enabled: bool,
    live: bool,
    theme: Theme,
    state: Arc<(Mutex<State>, Condvar)>,
    worker: Option<JoinHandle<()>>,
    events: EventBus,
    listener: Option<EventListenerId>,
}

impl TextProgress {
    /// Subscribe to actual lifecycle counts and refresh a compact terminal panel.
    pub(crate) fn new(
        enabled: bool,
        theme: Theme,
        cancellation: &CancellationToken,
        events: &EventBus,
        config: &spaceprune::api::ResolvedConfig,
    ) -> Self {
        let live = enabled
            && io::stderr().is_terminal()
            && std::env::var_os("TERM").is_none_or(|term| term != "dumb");
        let state = Arc::new((
            Mutex::new(State {
                module_names: config
                    .modules
                    .iter()
                    .map(|(id, module)| (id.clone(), module.name.clone()))
                    .collect(),
                ..State::default()
            }),
            Condvar::new(),
        ));
        let listener = enabled.then(|| {
            let state = Arc::clone(&state);
            events.subscribe_fn(move |envelope| {
                let (lock, wake) = &*state;
                lock.lock()
                    .expect("progress lock poisoned")
                    .on_event(&envelope.event, theme, live);
                wake.notify_one();
            })
        });
        let worker = live.then(|| {
            let state = Arc::clone(&state);
            let cancellation = cancellation.clone();
            thread::spawn(move || {
                let (lock, wake) = &*state;
                let mut state = lock.lock().expect("progress lock poisoned");
                while !state.shutdown {
                    if state.activity.is_some() && state.input_depth == 0 {
                        let _ = state.draw(theme, cancellation.is_cancelled());
                        state = wake
                            .wait_timeout(state, Duration::from_millis(100))
                            .expect("progress lock poisoned")
                            .0;
                    } else {
                        state = wake.wait(state).expect("progress lock poisoned");
                    }
                }
            })
        });
        Self {
            enabled,
            live,
            theme,
            state,
            worker,
            events: events.clone(),
            listener,
        }
    }
    /// Return the completed cleanup duration without emitting a second result summary.
    pub(crate) fn cleanup_elapsed(&self) -> Option<Duration> {
        self.state
            .0
            .lock()
            .expect("progress lock poisoned")
            .cleanup_elapsed
    }
}

impl ProgressSink for TextProgress {
    fn record_cleanup_with_accounting(
        &self,
        task: &spaceprune::api::TaskRequest,
        accounting: spaceprune::api::CleanupSizeAccounting,
        succeeded: usize,
        bytes: u64,
        unknown: usize,
    ) {
        if accounting != spaceprune::api::CleanupSizeAccounting::ProcessedBytes {
            if let Some(activity) = &mut self
                .state
                .0
                .lock()
                .expect("progress lock poisoned")
                .activity
            {
                activity
                    .completed_items
                    .insert((task.module_id.clone(), task.task_id.clone()), succeeded);
                if accounting == spaceprune::api::CleanupSizeAccounting::MemoryBytes {
                    activity.memory_sizes.insert(
                        (task.module_id.clone(), task.task_id.clone()),
                        (bytes, unknown),
                    );
                }
            }
        } else {
            self.record_cleanup(task, succeeded, bytes, unknown);
        }
    }
    fn record_scan(
        &self,
        task: &spaceprune::api::TaskRequest,
        measurement: spaceprune::api::ScanMeasurement,
    ) {
        if !self.live {
            return;
        }
        if let Some(activity) = &mut self
            .state
            .0
            .lock()
            .expect("progress lock poisoned")
            .activity
        {
            activity
                .measurements
                .insert((task.module_id.clone(), task.task_id.clone()), measurement);
        }
    }
    fn record_cleanup(
        &self,
        task: &spaceprune::api::TaskRequest,
        succeeded: usize,
        bytes: u64,
        unknown: usize,
    ) {
        if !self.live {
            return;
        }
        let (lock, _) = &*self.state;
        if let Some(activity) = &mut lock.lock().expect("progress lock poisoned").activity {
            activity
                .completed_items
                .insert((task.module_id.clone(), task.task_id.clone()), succeeded);
            activity.cleanup_sizes.insert(
                (task.module_id.clone(), task.task_id.clone()),
                (bytes, unknown),
            );
        }
    }
    fn emit(&self, event: ProgressEvent) {
        if !self.enabled {
            return;
        }
        if self.live {
            let (lock, _) = &*self.state;
            if let Some(activity) = &mut lock.lock().expect("progress lock poisoned").activity {
                activity.record(event);
            }
            // The ticker coalesces frequent filesystem updates instead of redrawing per file.
        } else {
            let prefix = match (event.current, event.total) {
                (Some(current), Some(total)) => format!("[{current}/{total}] "),
                _ => String::new(),
            };
            eprintln!("{prefix}{}", display_message(&event.message));
        }
    }

    fn set_input_active(&self, active: bool) {
        if !self.live {
            return;
        }
        let (lock, wake) = &*self.state;
        let mut state = lock.lock().expect("progress lock poisoned");
        if active {
            if state.input_depth == 0 {
                let _ = state.screen.restore();
                let mut output = io::stderr().lock();
                let stage =
                    state
                        .activity
                        .as_ref()
                        .map_or("", |activity| match activity.operation {
                            Operation::Scan => crate::i18n::text("terminal.scan_stage"),
                            Operation::Clean => crate::i18n::text("terminal.cleanup_stage"),
                        });
                let receipt = Receipt::stderr(self.theme);
                let _ = write!(
                    output,
                    "{}",
                    receipt.heading(
                        &crate::i18n::message(
                            "terminal.authorization_required",
                            &[("stage", stage.to_owned())]
                        ),
                        Tone::Accent,
                        None,
                    )
                );
                if let Some(event) = state
                    .activity
                    .as_ref()
                    .and_then(|activity| activity.event.as_ref())
                {
                    let _ = writeln!(output, "  {}", clip(&event.message, usize::MAX));
                }
                let _ = writeln!(
                    output,
                    "  {}",
                    self.theme.stderr(
                        crate::i18n::text("terminal.credentials_prompt"),
                        Tone::Muted
                    )
                );

                #[cfg(target_os = "windows")]
                let _ = writeln!(
                    output,
                    "  {}\n",
                    self.theme
                        .stderr(crate::i18n::text("terminal.windows_prompt"), Tone::Muted)
                );
                let _ = output.flush();
            }
            state.input_depth += 1;
        } else {
            state.input_depth = state.input_depth.saturating_sub(1);
        }
        wake.notify_one();
    }
}

impl Drop for TextProgress {
    fn drop(&mut self) {
        if let Some(listener) = self.listener {
            self.events.unsubscribe(listener);
        }
        let (lock, wake) = &*self.state;
        {
            let mut state = lock.lock().expect("progress lock poisoned");
            state.shutdown = true;
            if self.live {
                let _ = state.screen.restore();
            }
        }
        wake.notify_one();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub(crate) fn duration(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    format!("{:02}:{:02}", seconds / 60, seconds % 60)
}

fn clip(value: &str, width: usize) -> String {
    let value: String = value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect();
    if value.width() <= width {
        return value;
    }
    let mut result = String::new();
    let mut used = 0;
    for character in value.chars() {
        let size = character.width().unwrap_or(0);
        if used + size > width.saturating_sub(1) {
            break;
        }
        result.push(character);
        used += size;
    }
    if width > 0 {
        result.push('…');
    }
    result
}

/// Windows extended-path syntax is an I/O detail, even in embedded progress messages.
pub(crate) fn display_message(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut tail = value;
    while let Some(index) = tail.find(r"\\?\") {
        result.push_str(&tail[..index]);
        let suffix = &tail[index + 4..];
        if suffix.starts_with("UNC\\") || suffix.starts_with("unc\\") {
            result.push_str(r"\\");
            tail = &suffix[4..];
        } else if suffix
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic)
            && suffix.as_bytes().get(1..3) == Some(b":\\")
        {
            tail = suffix;
        } else {
            result.push_str(r"\\?\");
            tail = suffix;
        }
    }
    result.push_str(tail);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displayed_paths_hide_windows_transport_prefixes_but_keep_other_paths() {
        assert_eq!(
            display_message(r"Checking \\?\C:\cache\中文 file; \\?\UNC\server\share\file"),
            r"Checking C:\cache\中文 file; \\server\share\file"
        );
        assert_eq!(
            display_message(r"/home/cache/file \\?\Volume{test}\file"),
            r"/home/cache/file \\?\Volume{test}\file"
        );
    }

    #[test]
    fn memory_progress_is_separate_from_processed_disk_bytes() {
        let mut activity = Activity::new(Operation::Clean, 2);
        activity
            .cleanup_sizes
            .insert(("disk".into(), "files".into()), (1024, 0));
        activity
            .memory_sizes
            .insert(("ram".into(), "process".into()), (8192, 1));
        let summary = activity.size_summary();
        assert!(summary.contains("1.00 KiB") && summary.contains("8.00 KiB"));
        assert!(!summary.contains("9.00 KiB"));
        activity.cleanup_sizes.clear();
        assert!(!activity.size_summary().contains("0 B"));
    }

    #[test]
    fn cumulative_size_updates_replace_one_task_without_erasing_other_tasks() {
        let mut activity = Activity::new(Operation::Scan, 0);
        let key = ("cache".into(), "one".into());
        activity.measurements.insert(
            key.clone(),
            spaceprune::api::ScanMeasurement {
                checked_items: 1,
                logical_bytes: 10,
                ..Default::default()
            },
        );
        activity.measurements.insert(
            ("cache".into(), "two".into()),
            spaceprune::api::ScanMeasurement {
                checked_items: 1,
                logical_bytes: 20,
                ..Default::default()
            },
        );
        activity.measurements.insert(
            key,
            spaceprune::api::ScanMeasurement {
                checked_items: 2,
                logical_bytes: 30,
                unknown_size_items: 1,
                ..Default::default()
            },
        );
        let summary = activity.size_summary();
        assert!(summary.contains("50 B"), "{summary}");
        assert!(summary.contains("Checked 3"), "{summary}");
    }

    #[test]
    fn long_running_steps_keep_time_and_cancellation_visible_without_inventing_progress() {
        let mut activity = Activity::new(Operation::Clean, 4);
        activity.event = Some(ProgressEvent {
            phase: ProgressPhase::Clean,
            module_id: None,
            task_id: None,
            handler_id: None,
            current: None,
            total: Some(4),
            message: "Waiting for Btrfs synchronization (--sync)".to_owned(),
        });
        let now = activity.started + Duration::from_secs(65);
        let lines = activity.lines(now, false, 100);
        assert!(lines[0].0.contains("01:05"));
        assert!(lines[1].0.contains("0/4 · 0%"));
        assert!(lines[3].0.contains("Btrfs synchronization"));
        assert!(lines[4].0.is_empty());
        assert!(lines[5].0.contains("no new progress for 01:05"));
        assert!(
            activity.lines(now, true, 100)[5]
                .0
                .contains("Stop requested")
        );
        assert!(
            activity.lines(now, true, 100)[5]
                .0
                .contains("current operation")
        );
    }

    #[test]
    fn overall_scan_progress_does_not_finish_when_the_last_task_only_starts() {
        let mut activity = Activity::new(Operation::Scan, 77);
        activity.total_checks = Some(37);
        activity.checks = 36;
        activity.event = Some(ProgressEvent {
            phase: ProgressPhase::Measure,
            module_id: None,
            task_id: None,
            handler_id: None,
            current: Some(4),
            total: Some(4),
            message: "Scanning home snapshot #1074".into(),
        });
        let lines = activity.lines(activity.started, false, 100);
        assert!(lines[1].0.contains("Total checks 36/37 · 97%"));
        assert!(lines[1].0.contains('█'));
        assert!(lines[1].0.contains('░'));
        assert!(lines[3].0.contains("snapshot #1074"));
        let unknown = Activity::new(Operation::Scan, 0).progress_bar(Duration::ZERO, 100);
        assert!(unknown.contains("progress indeterminate"));
        assert!(!unknown.contains('%'));
    }

    #[test]
    fn activity_history_scrolls_recent_paths_and_keeps_the_live_counters() {
        let mut activity = Activity::new(Operation::Scan, 0);
        for index in 0..8 {
            activity.record(ProgressEvent {
                phase: ProgressPhase::Measure,
                module_id: Some("cache".into()),
                task_id: Some("files".into()),
                handler_id: None,
                current: Some(index),
                total: Some(8),
                message: format!("Measuring candidate metadata /fixture/{index}.log"),
            });
        }
        let history = activity.history_lines(5);
        assert_eq!(history.len(), 5);
        assert!(history[0].0.ends_with("/fixture/3.log"));
        assert!(history[4].0.ends_with("/fixture/7.log"));
        assert!(
            history[..4]
                .iter()
                .all(|(line, tone)| line.starts_with("  ") && matches!(tone, Tone::Muted))
        );
        assert!(history[4].0.starts_with("› "));
        assert!(matches!(history[4].1, Tone::Accent));
        assert!(activity.history_lines(0).is_empty());
        assert_eq!(activity.event.as_ref().unwrap().current, Some(7));
        assert_eq!(activity.active_module.as_deref(), Some("cache"));
    }

    #[test]
    fn current_activity_moves_on_and_completion_is_not_highlighted_as_running() {
        for (operation, phase) in [
            (Operation::Scan, ProgressPhase::Measure),
            (Operation::Clean, ProgressPhase::Clean),
        ] {
            let mut activity = Activity::new(operation, 1);
            let mut event = ProgressEvent {
                phase: ProgressPhase::Authorize,
                module_id: Some("fixture".into()),
                task_id: Some("cache".into()),
                handler_id: None,
                current: None,
                total: None,
                message: "Waiting for system authorization".into(),
            };
            activity.record(event.clone());
            let history = activity.history_lines(5);
            assert!(history[0].0.starts_with("› "));
            assert!(matches!(history[0].1, Tone::Warning));

            event.phase = phase;
            event.message = "Working on selected cache".into();
            activity.record(event.clone());
            let history = activity.history_lines(5);
            assert!(matches!(history[0].1, Tone::Muted));
            assert!(history[1].0.starts_with("› "));
            assert!(matches!(history[1].1, Tone::Accent));

            event.phase = ProgressPhase::Complete;
            event.message = "Finished".into();
            activity.record(event);
            assert!(
                activity
                    .history_lines(5)
                    .iter()
                    .all(|(line, tone)| line.starts_with("  ") && matches!(tone, Tone::Muted))
            );
        }
    }

    #[test]
    fn localized_progress_leaves_room_for_the_module_elapsed_time() {
        for width in [26, 64, 90] {
            let bar = render_bar("模块 系统快照恢复点", None, Duration::from_secs(76), width);
            assert!(bar.width() <= width, "{width}: {bar}");
            assert!(!bar.contains('%'));
        }
    }

    #[test]
    fn terminal_text_stays_on_one_line_and_respects_wide_characters() {
        assert_eq!(clip("项目名称", 5), "项目…");
        assert_eq!(clip("a\nb\r\t\u{1b}c", 80), "a b   c");
        assert_eq!(clip("long", 0), "");
        assert_eq!(clip("long", 1), "…");
    }
}
