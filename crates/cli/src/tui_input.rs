use anyhow::Result;
use crossterm::{
    event::{self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyModifiers},
    style::Print,
    terminal::{Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
    QueueableCommand,
};
use std::io::{self, Stdin, Stdout, Write};
use std::collections::VecDeque;

const MAX_HISTORY: usize = 100;

pub struct TuiInput {
    stdout: Stdout,
    stdin: Stdin,
    history: VecDeque<String>,
    history_index: Option<usize>,
}

impl TuiInput {
    pub fn new() -> Self {
        Self {
            stdout: io::stdout(),
            stdin: io::stdin(),
            history: VecDeque::new(),
            history_index: None,
        }
    }

    /// Initialize terminal for TUI mode
    pub fn enable_raw_mode(&mut self) -> Result<()> {
        use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
        
        // Enable raw mode directly
        enable_raw_mode()?;
        
        // Enter alternate screen
        crossterm::execute!(self.stdout, EnterAlternateScreen)?;
        
        // Enable bracketed paste mode
        crossterm::execute!(self.stdout, EnableBracketedPaste)?;
        
        self.stdout.flush()?;
        Ok(())
    }

    /// Disable raw mode and restore terminal
    pub fn disable_raw_mode(&mut self) -> Result<()> {
        use crossterm::terminal::{disable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
        
        crossterm::execute!(self.stdout, LeaveAlternateScreen)?;
        disable_raw_mode()?;
        self.stdout.flush()?;
        Ok(())
    }

    /// Read a single line with full line editing support
    pub fn read_line(&mut self, prompt: &str) -> Result<Option<String>> {
        self.stdout.queue(Print(prompt))?;
        self.stdout.flush()?;

        let mut buffer = String::new();
        let mut cursor_pos = 0;

        loop {
            // Wait for next event with timeout
            if let Event::Key(key) = event::read()? {
                match key.code {
                    KeyCode::Enter => {
                        // Submit input
                        crossterm::execute!(self.stdout, Print("\r\n"))?;
                        self.stdout.flush()?;

                        if !buffer.is_empty() {
                            // Add to history (avoid duplicates)
                            if self.history.front() != Some(&buffer) {
                                self.history.push_front(buffer.clone());
                                if self.history.len() > MAX_HISTORY {
                                    self.history.pop_back();
                                }
                            }
                            return Ok(Some(buffer));
                        }
                        return Ok(Some(String::new()));
                    }
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        crossterm::execute!(self.stdout, Print("^C\r\n"))?;
                        self.stdout.flush()?;
                        return Ok(None);
                    }
                    KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                        crossterm::execute!(self.stdout, Print("^D\r\n"))?;
                        self.stdout.flush()?;
                        return Ok(None);
                    }
                    KeyCode::Backspace => {
                        if cursor_pos > 0 {
                            cursor_pos -= 1;
                            buffer.remove(cursor_pos);
                            self.redraw_line(prompt, &buffer, cursor_pos)?;
                        }
                    }
                    KeyCode::Delete => {
                        if cursor_pos < buffer.len() {
                            buffer.remove(cursor_pos);
                            self.redraw_line(prompt, &buffer, cursor_pos)?;
                        }
                    }
                    KeyCode::Left => {
                        if cursor_pos > 0 {
                            cursor_pos -= 1;
                            self.move_cursor_left(1)?;
                        }
                    }
                    KeyCode::Right => {
                        if cursor_pos < buffer.len() {
                            cursor_pos += 1;
                            self.move_cursor_right(1)?;
                        }
                    }
                    KeyCode::Home => {
                        if cursor_pos > 0 {
                            let moves = cursor_pos;
                            cursor_pos = 0;
                            self.move_cursor_left(moves)?;
                        }
                    }
                    KeyCode::End => {
                        if cursor_pos < buffer.len() {
                            let moves = buffer.len() - cursor_pos;
                            cursor_pos = buffer.len();
                            self.move_cursor_right(moves)?;
                        }
                    }
                    KeyCode::Up => {
                        // Navigate history up
                        if let Some(idx) = self.history_index {
                            if idx < self.history.len() - 1 {
                                let new_idx = idx + 1;
                                self.history_index = Some(new_idx);
                                let history_item = self.history[new_idx].clone();
                                self.clear_and_redraw(prompt, &history_item)?;
                                buffer = history_item;
                                cursor_pos = buffer.len();
                            }
                        } else if !self.history.is_empty() {
                            self.history_index = Some(0);
                            let history_item = self.history[0].clone();
                            self.clear_and_redraw(prompt, &history_item)?;
                            buffer = history_item;
                            cursor_pos = buffer.len();
                        }
                    }
                    KeyCode::Down => {
                        // Navigate history down
                        if let Some(idx) = self.history_index {
                            if idx == 0 {
                                self.history_index = None;
                                self.clear_and_redraw(prompt, "")?;
                                buffer.clear();
                                cursor_pos = 0;
                            } else {
                                let new_idx = idx - 1;
                                self.history_index = Some(new_idx);
                                let history_item = self.history[new_idx].clone();
                                self.clear_and_redraw(prompt, &history_item)?;
                                buffer = history_item;
                                cursor_pos = buffer.len();
                            }
                        }
                    }
                    KeyCode::Tab => {
                        // TODO: Implement tab completion
                        // For now, just insert spaces
                        buffer.insert(cursor_pos, ' ');
                        cursor_pos += 1;
                        self.redraw_line(prompt, &buffer, cursor_pos)?;
                    }
                    KeyCode::Char(c) => {
                        if key.modifiers.contains(KeyModifiers::CONTROL) {
                            // Handle Ctrl+key combinations
                            match c {
                                'a' => {
                                    cursor_pos = 0;
                                    self.move_cursor_left(buffer.len())?;
                                }
                                'e' => {
                                    let moves = buffer.len() - cursor_pos;
                                    cursor_pos = buffer.len();
                                    self.move_cursor_right(moves)?;
                                }
                                'k' => {
                                    // Delete from cursor to end
                                    buffer.truncate(cursor_pos);
                                    self.redraw_line(prompt, &buffer, cursor_pos)?;
                                }
                                'u' => {
                                    // Delete from start to cursor
                                    buffer.drain(..cursor_pos);
                                    cursor_pos = 0;
                                    self.redraw_line(prompt, &buffer, cursor_pos)?;
                                }
                                'w' => {
                                    // Delete word before cursor
                                    let start = buffer[..cursor_pos]
                                        .trim_end()
                                        .rfind(|c: char| c.is_whitespace())
                                        .map(|i| i + 1)
                                        .unwrap_or(0);
                                    buffer.drain(start..cursor_pos);
                                    cursor_pos = start;
                                    self.redraw_line(prompt, &buffer, cursor_pos)?;
                                }
                                _ => {}
                            }
                        } else {
                            buffer.insert(cursor_pos, c);
                            cursor_pos += 1;
                            self.redraw_line(prompt, &buffer, cursor_pos)?;
                        }
                    }
                    KeyCode::Esc => {
                        // Handle escape sequences for some terminals
                        if let Event::Key(KeyEvent {
                            code: KeyCode::Char('3'),
                            modifiers: _,
                            ..
                        }) = event::read()? {
                            // Delete key sends Esc[3~
                            if cursor_pos < buffer.len() {
                                buffer.remove(cursor_pos);
                                self.redraw_line(prompt, &buffer, cursor_pos)?;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    fn redraw_line(&mut self, prompt: &str, buffer: &str, cursor_pos: usize) -> Result<()> {
        // Move cursor to start of line
        crossterm::execute!(self.stdout, crossterm::cursor::MoveToColumn(0))?;
        crossterm::execute!(self.stdout, Clear(ClearType::CurrentLine))?;
        crossterm::execute!(self.stdout, Print(prompt))?;
        crossterm::execute!(self.stdout, Print(buffer))?;

        // Move cursor back to correct position
        let cursor_col = prompt.len() + cursor_pos;
        crossterm::execute!(self.stdout, crossterm::cursor::MoveToColumn(cursor_col as u16))?;
        self.stdout.flush()?;

        Ok(())
    }

    fn clear_and_redraw(&mut self, prompt: &str, buffer: &str) -> Result<()> {
        // Clear entire line
        crossterm::execute!(self.stdout, crossterm::cursor::MoveToColumn(0))?;
        crossterm::execute!(self.stdout, Clear(ClearType::CurrentLine))?;
        crossterm::execute!(self.stdout, Print(prompt))?;
        crossterm::execute!(self.stdout, Print(buffer))?;

        let cursor_col = prompt.len() + buffer.len();
        crossterm::execute!(self.stdout, crossterm::cursor::MoveToColumn(cursor_col as u16))?;
        self.stdout.flush()?;

        Ok(())
    }

    fn move_cursor_left(&mut self, n: usize) -> Result<()> {
        // Get current cursor position and move left
        if let Ok(pos) = crossterm::cursor::position() {
            let new_col = pos.0.saturating_sub(n as u16);
            self.stdout.queue(crossterm::cursor::MoveTo(new_col, pos.1))?;
            self.stdout.flush()?;
        }
        Ok(())
    }

    fn move_cursor_right(&mut self, n: usize) -> Result<()> {
        if let Ok(pos) = crossterm::cursor::position() {
            let new_col = pos.0.saturating_add(n as u16);
            self.stdout.queue(crossterm::cursor::MoveTo(new_col, pos.1))?;
            self.stdout.flush()?;
        }
        Ok(())
    }

    /// Add a command to history manually (e.g., from file)
    pub fn add_to_history(&mut self, cmd: String) {
        if !cmd.is_empty() && self.history.front() != Some(&cmd) {
            self.history.push_front(cmd);
            if self.history.len() > MAX_HISTORY {
                self.history.pop_back();
            }
        }
    }

    /// Clear history
    pub fn clear_history(&mut self) {
        self.history.clear();
        self.history_index = None;
    }

    /// Get history as a vector
    pub fn get_history(&self) -> Vec<String> {
        self.history.iter().cloned().collect()
    }
}

impl Default for TuiInput {
    fn default() -> Self {
        Self::new()
    }
}