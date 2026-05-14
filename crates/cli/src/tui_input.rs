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
                            // Convert character position to byte position
                            let byte_pos = buffer
                                .char_indices()
                                .nth(cursor_pos - 1)
                                .map(|(pos, _)| pos)
                                .unwrap_or(0);
                            
                            cursor_pos -= 1;
                            
                            // Calculate byte length of the character to remove
                            let char_len = if cursor_pos < buffer.chars().count() {
                                buffer.chars().nth(cursor_pos).map(|c| c.len_utf8()).unwrap_or(1)
                            } else {
                                1
                            };
                            
                            buffer.drain(byte_pos..byte_pos + char_len);
                            self.redraw_line(prompt, &buffer, cursor_pos)?;
                        }
                    }
                    KeyCode::Delete => {
                        let char_count = buffer.chars().count();
                        if cursor_pos < char_count {
                            // Convert character position to byte position
                            let byte_pos = buffer
                                .char_indices()
                                .nth(cursor_pos)
                                .map(|(pos, _)| pos)
                                .unwrap_or(buffer.len());
                            
                            // Calculate byte length of the character to remove
                            let char_len = buffer
                                .chars()
                                .nth(cursor_pos)
                                .map(|c| c.len_utf8())
                                .unwrap_or(1);
                            
                            buffer.drain(byte_pos..byte_pos + char_len);
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
                        let char_count = buffer.chars().count();
                        if cursor_pos < char_count {
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
                        let char_count = buffer.chars().count();
                        if cursor_pos < char_count {
                            let moves = char_count - cursor_pos;
                            cursor_pos = char_count;
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
                                cursor_pos = buffer.chars().count();
                            }
                        } else if !self.history.is_empty() {
                            self.history_index = Some(0);
                            let history_item = self.history[0].clone();
                            self.clear_and_redraw(prompt, &history_item)?;
                            buffer = history_item;
                            cursor_pos = buffer.chars().count();
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
                                cursor_pos = buffer.chars().count();
                            }
                        }
                    }
                    KeyCode::Tab => {
                        // TODO: Implement tab completion
                        // For now, just insert spaces (convert char position to byte position)
                        let byte_pos = buffer
                            .char_indices()
                            .nth(cursor_pos)
                            .map(|(pos, _)| pos)
                            .unwrap_or(buffer.len());
                        
                        buffer.insert(byte_pos, ' ');
                        cursor_pos += 1;
                        self.redraw_line(prompt, &buffer, cursor_pos)?;
                    }
                    KeyCode::Char(c) => {
                        if key.modifiers.contains(KeyModifiers::CONTROL) {
                            // Handle Ctrl+key combinations
                            let char_count = buffer.chars().count();
                            match c {
                                'a' => {
                                    cursor_pos = 0;
                                    self.move_cursor_left(char_count)?;
                                }
                                'e' => {
                                    let moves = char_count - cursor_pos;
                                    cursor_pos = char_count;
                                    self.move_cursor_right(moves)?;
                                }
                                'k' => {
                                    // Delete from cursor to end
                                    // Convert char position to byte position
                                    let byte_pos = buffer
                                        .char_indices()
                                        .nth(cursor_pos)
                                        .map(|(pos, _)| pos)
                                        .unwrap_or(buffer.len());
                                    buffer.truncate(byte_pos);
                                    self.redraw_line(prompt, &buffer, cursor_pos)?;
                                }
                                'u' => {
                                    // Delete from start to cursor
                                    // Convert char position to byte position
                                    let byte_pos = buffer
                                        .char_indices()
                                        .nth(cursor_pos)
                                        .map(|(pos, _)| pos)
                                        .unwrap_or(buffer.len());
                                    buffer.drain(..byte_pos);
                                    cursor_pos = 0;
                                    self.redraw_line(prompt, &buffer, cursor_pos)?;
                                }
                                'w' => {
                                    // Delete word before cursor
                                    // Find the byte position of the character at cursor_pos
                                    let cursor_byte = buffer
                                        .char_indices()
                                        .nth(cursor_pos)
                                        .map(|(pos, _)| pos)
                                        .unwrap_or(buffer.len());
                                    
                                    // Find the word boundary (need to search from start)
                                    let mut word_start_byte = 0;
                                    let mut current_char_idx = 0;
                                    
                                    for (byte_idx, _) in buffer.char_indices() {
                                        if current_char_idx >= cursor_pos {
                                            break;
                                        }
                                        // Look backwards to find word boundary
                                        let remaining = &buffer[byte_idx..cursor_byte];
                                        if let Some(pos) = remaining.trim_end().rfind(|c: char| c.is_whitespace()) {
                                            word_start_byte = byte_idx + pos + 1;
                                            break;
                                        }
                                        current_char_idx += 1;
                                    }
                                    
                                    // If no word boundary found, delete from start
                                    if current_char_idx < cursor_pos {
                                        word_start_byte = 0;
                                    }
                                    
                                    buffer.drain(word_start_byte..cursor_byte);
                                    cursor_pos = buffer[..word_start_byte].chars().count();
                                    self.redraw_line(prompt, &buffer, cursor_pos)?;
                                }
                                _ => {}
                            }
                        } else {
                            // Convert character position to byte position
                            let byte_pos = buffer
                                .char_indices()
                                .nth(cursor_pos)
                                .map(|(pos, _)| pos)
                                .unwrap_or(buffer.len());
                            
                            buffer.insert(byte_pos, c);
                            // Move cursor after the inserted character (always +1 char)
                            cursor_pos += 1;
                            
                            self.redraw_line(prompt, &buffer, cursor_pos)?;
                        }
                    }
                    KeyCode::Esc => {
                        // Ignore standalone escape key presses
                        // (Delete key is handled by KeyCode::Delete above)
                    }
                    _ => {}
                }
            }
        }
    }

    fn redraw_line(&mut self, prompt: &str, buffer: &str, cursor_pos: usize) -> Result<()> {
        // Calculate visible width (handles Unicode properly)
        let prompt_width = Self::visible_width(prompt);
        let buffer_width = Self::visible_width(buffer);
        let cursor_col = prompt_width + Self::char_index_to_width(buffer, cursor_pos);
        
        // Move cursor to column 0
        crossterm::execute!(self.stdout, crossterm::cursor::MoveToColumn(0))?;
        // Clear the current line
        crossterm::execute!(self.stdout, Clear(ClearType::CurrentLine))?;
        // Print prompt and buffer
        crossterm::execute!(self.stdout, Print(prompt))?;
        crossterm::execute!(self.stdout, Print(buffer))?;
        // Move cursor to calculated position
        crossterm::execute!(self.stdout, crossterm::cursor::MoveToColumn(cursor_col as u16))?;
        self.stdout.flush()?;

        Ok(())
    }

    fn clear_and_redraw(&mut self, prompt: &str, buffer: &str) -> Result<()> {
        let prompt_width = Self::visible_width(prompt);
        let buffer_width = Self::visible_width(buffer);
        let cursor_col = prompt_width + buffer_width;

        crossterm::execute!(self.stdout, crossterm::cursor::MoveToColumn(0))?;
        crossterm::execute!(self.stdout, Clear(ClearType::CurrentLine))?;
        crossterm::execute!(self.stdout, Print(prompt))?;
        crossterm::execute!(self.stdout, Print(buffer))?;
        crossterm::execute!(self.stdout, crossterm::cursor::MoveToColumn(cursor_col as u16))?;
        self.stdout.flush()?;

        Ok(())
    }
    
    /// Calculate the visible width of a string (handles Unicode)
    /// Returns 2 for wide characters (CJK), 1 for others
    fn visible_width(s: &str) -> usize {
        s.chars().map(Self::char_width).sum()
    }
    
    /// Calculate the width of a single character
    /// CJK characters, emojis, etc. typically take 2 columns
    fn char_width(c: char) -> usize {
        // Check if character is wide (CJK, emoji, etc.)
        // Based on East Asian Width property
        if c as u32 >= 0x1100 && 
           (c as u32 <= 0x115F ||  // Hangul Jamo
            c as u32 == 0x2329 ||  // Left-pointing angle bracket
            c as u32 == 0x232A ||  // Right-pointing angle bracket
            c as u32 >= 0x2E80 && c as u32 <= 0x303E ||  // CJK Radicals
            c as u32 >= 0x3040 && c as u32 <= 0xA4CF ||  // Hiragana, Katakana, etc.
            c as u32 >= 0xAC00 && c as u32 <= 0xD7A3 ||  // Hangul Syllables
            c as u32 >= 0xF900 && c as u32 <= 0xFAFF ||  // CJK Compatibility Ideographs
            c as u32 >= 0xFE10 && c as u32 <= 0xFE1F ||  // Vertical forms
            c as u32 >= 0xFE30 && c as u32 <= 0xFE6F ||  // CJK Compatibility Forms
            c as u32 >= 0xFF00 && c as u32 <= 0xFF60 ||  // Fullwidth forms
            c as u32 >= 0xFFE0 && c as u32 <= 0xFFE6 ||  // Fullwidth forms
            c as u32 >= 0x20000 && c as u32 <= 0x2FFFD ||  // Supplementary
            c as u32 >= 0x30000 && c as u32 <= 0x3FFFD)  // Supplementary
        {
            2
        } else {
            1
        }
    }
    
    /// Calculate the width up to a specific character index
    fn char_index_to_width(s: &str, char_index: usize) -> usize {
        s.chars()
            .take(char_index)
            .map(Self::char_width)
            .sum()
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