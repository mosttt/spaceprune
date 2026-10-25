//! Keep redraws on an owned screen; leave cooked input and the original history alone.

use std::io::{self, Write};

use crossterm::{
    SynchronizedUpdate,
    cursor::{Hide, MoveTo, Show},
    execute, queue,
    terminal::{
        Clear, ClearType, EndSynchronizedUpdate, EnterAlternateScreen, LeaveAlternateScreen,
    },
};

use super::{Theme, Tone, clip};

#[derive(Default)]
pub(super) struct ProgressScreen {
    active: bool,
}

impl ProgressScreen {
    fn open() -> io::Result<Self> {
        // Restore even if entering the alternate screen only partially succeeds.
        let screen = Self { active: true };
        execute!(io::stderr(), EnterAlternateScreen)?;
        Ok(screen)
    }

    pub(super) fn draw(
        &mut self,
        lines: &[(String, Tone)],
        theme: Theme,
        columns: u16,
        rows: u16,
    ) -> io::Result<()> {
        if !self.active {
            *self = Self::open()?;
        }
        let mut output = io::stderr().lock();
        output.sync_update(|output| -> io::Result<()> {
            // Input can echo between frames. Repaint without cursor queries or reading stdin.
            queue!(output, Hide, Clear(ClearType::All))?;
            let width = usize::from(columns.saturating_sub(1));
            for (row, (line, tone)) in lines.iter().take(usize::from(rows)).enumerate() {
                queue!(output, MoveTo(0, row as u16))?;
                write!(output, "{}", theme.stderr(clip(line, width), *tone))?;
            }
            Ok(())
        })?
    }

    pub(super) fn restore(&mut self) -> io::Result<()> {
        if self.active {
            execute!(
                io::stderr(),
                EndSynchronizedUpdate,
                LeaveAlternateScreen,
                Show
            )?;
            self.active = false;
        }
        Ok(())
    }
}

impl Drop for ProgressScreen {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}
