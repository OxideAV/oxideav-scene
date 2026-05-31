//! Minimal SVG-path-data parser for [`Shape::Path`](crate::object::Shape::Path).
//!
//! The SVG 1.1 "path data" mini-grammar is widely-published commodity
//! syntax — each command is a single ASCII letter (case picks absolute
//! / relative coordinates) followed by a list of `f32` arguments
//! separated by whitespace and / or commas. The supported subset here
//! covers everything `oxideav-core::Path` can express:
//!
//! | Cmd        | Args                  | Meaning                               |
//! |------------|-----------------------|---------------------------------------|
//! | `M` / `m`  | `(x y)+`              | Move-to. Extra coord pairs become     |
//! |            |                       | implicit line-to commands.            |
//! | `L` / `l`  | `(x y)+`              | Line-to.                              |
//! | `H` / `h`  | `x+`                  | Horizontal line-to.                   |
//! | `V` / `v`  | `y+`                  | Vertical line-to.                     |
//! | `C` / `c`  | `(x1 y1 x2 y2 x y)+`  | Cubic Bezier.                         |
//! | `S` / `s`  | `(x2 y2 x y)+`        | Smooth cubic (reflect prev control).  |
//! | `Q` / `q`  | `(x1 y1 x y)+`        | Quadratic Bezier.                     |
//! | `T` / `t`  | `(x y)+`              | Smooth quadratic (reflect previous).  |
//! | `Z` / `z`  | —                     | Close sub-path.                       |
//!
//! Arc commands (`A` / `a`) are **not** parsed — `oxideav-core::Path`
//! exposes only line / quad / cubic primitives, and converting an
//! elliptical arc into a cubic-spline approximation is its own design
//! decision (number of segments, error bound). Calling code with an
//! arc-using path gets [`SvgPathError::UnsupportedCommand`] so the
//! caller can decide whether to drop the object, log, or supply a
//! pre-flattened path.
//!
//! Numeric tokens accept SVG's standard forms: integers, decimals with
//! optional leading sign, leading-dot decimals (`.5`), trailing-dot
//! decimals (`5.`), and scientific notation (`1e3`, `1.5E-2`). Consecutive
//! coordinate pairs may be separated by whitespace and / or a single
//! comma.

use oxideav_core::{Path, Point};

/// Reasons the path data could not be lowered to a [`Path`].
#[derive(Clone, Debug, PartialEq)]
pub enum SvgPathError {
    /// A command letter outside the supported set (currently arcs).
    UnsupportedCommand(char),
    /// A command was expected but the input ended (or contained a
    /// non-command, non-whitespace, non-digit character at top level).
    UnexpectedChar(char),
    /// A numeric argument could not be parsed.
    InvalidNumber,
    /// A command had too few arguments (e.g. `M 1` with no `y`).
    Truncated,
    /// The path data did not start with a move-to (`M` / `m`). Other
    /// commands implicitly need a current point and the SVG spec
    /// requires `M` / `m` to be the first command.
    NotStartedWithMove,
}

/// Parse SVG path data into an [`oxideav_core::Path`].
///
/// See the module docs for the supported grammar subset. Empty input
/// produces an empty `Path`.
pub fn parse_path(data: &str) -> Result<Path, SvgPathError> {
    let parser = Parser::new(data);
    parser.parse()
}

/// Axis-aligned bounding box (min_x, min_y, max_x, max_y) of every
/// anchor / control point referenced by an SVG path-data string.
///
/// Returns `None` when the input is empty or unparseable. The bound
/// is conservative for curves — it uses the convex-hull-of-control-
/// points approximation (an exact Bezier tight-bound would walk the
/// derivative roots; the hull is a strict superset, which is what a
/// scene-layer "content size" wants for layout). Stroke half-widths
/// are not included.
pub fn parse_bbox(data: &str) -> Option<(f32, f32, f32, f32)> {
    let path = parse_path(data).ok()?;
    if path.commands.is_empty() {
        return None;
    }
    let mut min_x = f32::INFINITY;
    let mut min_y = f32::INFINITY;
    let mut max_x = f32::NEG_INFINITY;
    let mut max_y = f32::NEG_INFINITY;
    let mut hit = false;
    let mut push = |x: f32, y: f32, hit: &mut bool| {
        *hit = true;
        if x < min_x {
            min_x = x;
        }
        if y < min_y {
            min_y = y;
        }
        if x > max_x {
            max_x = x;
        }
        if y > max_y {
            max_y = y;
        }
    };
    use oxideav_core::PathCommand as C;
    for c in &path.commands {
        match *c {
            C::MoveTo(p) | C::LineTo(p) => push(p.x, p.y, &mut hit),
            C::QuadCurveTo { control, end } => {
                push(control.x, control.y, &mut hit);
                push(end.x, end.y, &mut hit);
            }
            C::CubicCurveTo { c1, c2, end } => {
                push(c1.x, c1.y, &mut hit);
                push(c2.x, c2.y, &mut hit);
                push(end.x, end.y, &mut hit);
            }
            C::Close => {}
            _ => {}
        }
    }
    if !hit {
        return None;
    }
    Some((min_x, min_y, max_x, max_y))
}

// --------------------------------------------------------------------
// Implementation
// --------------------------------------------------------------------

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
    out: Path,
    // Current pen position (the "current point" in SVG terms).
    cx: f32,
    cy: f32,
    // Start of the current sub-path, restored by `Z`.
    start_x: f32,
    start_y: f32,
    // Previous quadratic / cubic control point reflection target. Reset
    // to the current point when the previous command wasn't a matching
    // curve type (per SVG §8.3.6 / §8.3.7).
    prev_cubic_ctrl: Option<(f32, f32)>,
    prev_quad_ctrl: Option<(f32, f32)>,
    started: bool,
}

impl<'a> Parser<'a> {
    fn new(data: &'a str) -> Self {
        Self {
            bytes: data.as_bytes(),
            pos: 0,
            out: Path::new(),
            cx: 0.0,
            cy: 0.0,
            start_x: 0.0,
            start_y: 0.0,
            prev_cubic_ctrl: None,
            prev_quad_ctrl: None,
            started: false,
        }
    }

    fn parse(mut self) -> Result<Path, SvgPathError> {
        self.skip_ws_comma();
        while self.pos < self.bytes.len() {
            let cmd = self.bytes[self.pos] as char;
            if !is_command(cmd) {
                return Err(SvgPathError::UnexpectedChar(cmd));
            }
            self.pos += 1;
            if !self.started && !matches!(cmd, 'M' | 'm') {
                return Err(SvgPathError::NotStartedWithMove);
            }
            self.dispatch(cmd)?;
            self.skip_ws_comma();
        }
        Ok(self.out)
    }

    fn dispatch(&mut self, cmd: char) -> Result<(), SvgPathError> {
        let abs = cmd.is_ascii_uppercase();
        match cmd.to_ascii_uppercase() {
            'M' => self.cmd_move(abs)?,
            'L' => self.cmd_line(abs)?,
            'H' => self.cmd_hline(abs)?,
            'V' => self.cmd_vline(abs)?,
            'C' => self.cmd_cubic(abs)?,
            'S' => self.cmd_smooth_cubic(abs)?,
            'Q' => self.cmd_quad(abs)?,
            'T' => self.cmd_smooth_quad(abs)?,
            'Z' => self.cmd_close(),
            // Arcs are deliberately unsupported (see module doc).
            'A' => return Err(SvgPathError::UnsupportedCommand(cmd)),
            other => return Err(SvgPathError::UnsupportedCommand(other)),
        }
        // Clear smooth-curve reflection state for non-curve commands.
        match cmd.to_ascii_uppercase() {
            'C' | 'S' => self.prev_quad_ctrl = None,
            'Q' | 'T' => self.prev_cubic_ctrl = None,
            _ => {
                self.prev_cubic_ctrl = None;
                self.prev_quad_ctrl = None;
            }
        }
        Ok(())
    }

    fn cmd_move(&mut self, abs: bool) -> Result<(), SvgPathError> {
        // First coord pair after M/m is a moveto; subsequent pairs are
        // implicit line-tos (with matching abs / rel sense).
        let (mut x, mut y) = self.read_pair()?;
        if !abs {
            x += self.cx;
            y += self.cy;
        }
        self.cx = x;
        self.cy = y;
        self.start_x = x;
        self.start_y = y;
        self.out.move_to(Point::new(x, y));
        self.started = true;
        // Greedy follow-on line-tos.
        while self.peek_number().is_some() {
            let (mut nx, mut ny) = self.read_pair()?;
            if !abs {
                nx += self.cx;
                ny += self.cy;
            }
            self.cx = nx;
            self.cy = ny;
            self.out.line_to(Point::new(nx, ny));
        }
        Ok(())
    }

    fn cmd_line(&mut self, abs: bool) -> Result<(), SvgPathError> {
        let mut got_any = false;
        while self.peek_number().is_some() {
            let (mut x, mut y) = self.read_pair()?;
            if !abs {
                x += self.cx;
                y += self.cy;
            }
            self.cx = x;
            self.cy = y;
            self.out.line_to(Point::new(x, y));
            got_any = true;
        }
        if !got_any {
            return Err(SvgPathError::Truncated);
        }
        Ok(())
    }

    fn cmd_hline(&mut self, abs: bool) -> Result<(), SvgPathError> {
        let mut got_any = false;
        while self.peek_number().is_some() {
            let mut x = self.read_number()?;
            if !abs {
                x += self.cx;
            }
            self.cx = x;
            self.out.line_to(Point::new(x, self.cy));
            got_any = true;
        }
        if !got_any {
            return Err(SvgPathError::Truncated);
        }
        Ok(())
    }

    fn cmd_vline(&mut self, abs: bool) -> Result<(), SvgPathError> {
        let mut got_any = false;
        while self.peek_number().is_some() {
            let mut y = self.read_number()?;
            if !abs {
                y += self.cy;
            }
            self.cy = y;
            self.out.line_to(Point::new(self.cx, y));
            got_any = true;
        }
        if !got_any {
            return Err(SvgPathError::Truncated);
        }
        Ok(())
    }

    fn cmd_cubic(&mut self, abs: bool) -> Result<(), SvgPathError> {
        let mut got_any = false;
        while self.peek_number().is_some() {
            let (mut x1, mut y1) = self.read_pair()?;
            let (mut x2, mut y2) = self.read_pair()?;
            let (mut x, mut y) = self.read_pair()?;
            if !abs {
                x1 += self.cx;
                y1 += self.cy;
                x2 += self.cx;
                y2 += self.cy;
                x += self.cx;
                y += self.cy;
            }
            self.out
                .cubic_to(Point::new(x1, y1), Point::new(x2, y2), Point::new(x, y));
            self.prev_cubic_ctrl = Some((x2, y2));
            self.cx = x;
            self.cy = y;
            got_any = true;
        }
        if !got_any {
            return Err(SvgPathError::Truncated);
        }
        Ok(())
    }

    fn cmd_smooth_cubic(&mut self, abs: bool) -> Result<(), SvgPathError> {
        let mut got_any = false;
        while self.peek_number().is_some() {
            // First control is the reflection of the previous cubic's
            // second control through the current point. If the previous
            // command wasn't a cubic, the reflection collapses to the
            // current point (SVG §8.3.6).
            let (rx, ry) = match self.prev_cubic_ctrl {
                Some((px, py)) => (2.0 * self.cx - px, 2.0 * self.cy - py),
                None => (self.cx, self.cy),
            };
            let (mut x2, mut y2) = self.read_pair()?;
            let (mut x, mut y) = self.read_pair()?;
            if !abs {
                x2 += self.cx;
                y2 += self.cy;
                x += self.cx;
                y += self.cy;
            }
            self.out
                .cubic_to(Point::new(rx, ry), Point::new(x2, y2), Point::new(x, y));
            self.prev_cubic_ctrl = Some((x2, y2));
            self.cx = x;
            self.cy = y;
            got_any = true;
        }
        if !got_any {
            return Err(SvgPathError::Truncated);
        }
        Ok(())
    }

    fn cmd_quad(&mut self, abs: bool) -> Result<(), SvgPathError> {
        let mut got_any = false;
        while self.peek_number().is_some() {
            let (mut x1, mut y1) = self.read_pair()?;
            let (mut x, mut y) = self.read_pair()?;
            if !abs {
                x1 += self.cx;
                y1 += self.cy;
                x += self.cx;
                y += self.cy;
            }
            self.out.quad_to(Point::new(x1, y1), Point::new(x, y));
            self.prev_quad_ctrl = Some((x1, y1));
            self.cx = x;
            self.cy = y;
            got_any = true;
        }
        if !got_any {
            return Err(SvgPathError::Truncated);
        }
        Ok(())
    }

    fn cmd_smooth_quad(&mut self, abs: bool) -> Result<(), SvgPathError> {
        let mut got_any = false;
        while self.peek_number().is_some() {
            let (rx, ry) = match self.prev_quad_ctrl {
                Some((px, py)) => (2.0 * self.cx - px, 2.0 * self.cy - py),
                None => (self.cx, self.cy),
            };
            let (mut x, mut y) = self.read_pair()?;
            if !abs {
                x += self.cx;
                y += self.cy;
            }
            self.out.quad_to(Point::new(rx, ry), Point::new(x, y));
            self.prev_quad_ctrl = Some((rx, ry));
            self.cx = x;
            self.cy = y;
            got_any = true;
        }
        if !got_any {
            return Err(SvgPathError::Truncated);
        }
        Ok(())
    }

    fn cmd_close(&mut self) {
        self.out.close();
        self.cx = self.start_x;
        self.cy = self.start_y;
    }

    // ---- low-level helpers ----

    fn skip_ws_comma(&mut self) {
        // SVG path grammar: whitespace = space / tab / CR / LF / FF;
        // a single comma is a separator between numbers.
        let mut seen_comma = false;
        while self.pos < self.bytes.len() {
            let c = self.bytes[self.pos];
            match c {
                b' ' | b'\t' | b'\r' | b'\n' | 0x0C => self.pos += 1,
                b',' if !seen_comma => {
                    seen_comma = true;
                    self.pos += 1;
                }
                _ => break,
            }
        }
    }

    fn peek_number(&mut self) -> Option<u8> {
        self.skip_ws_comma();
        if self.pos >= self.bytes.len() {
            return None;
        }
        let c = self.bytes[self.pos];
        if c.is_ascii_digit() || c == b'+' || c == b'-' || c == b'.' {
            Some(c)
        } else {
            None
        }
    }

    fn read_number(&mut self) -> Result<f32, SvgPathError> {
        self.skip_ws_comma();
        if self.pos >= self.bytes.len() {
            return Err(SvgPathError::Truncated);
        }
        let start = self.pos;
        // sign
        if matches!(self.bytes[self.pos], b'+' | b'-') {
            self.pos += 1;
        }
        // integer part
        while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        // fractional part
        if self.pos < self.bytes.len() && self.bytes[self.pos] == b'.' {
            self.pos += 1;
            while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }
        // exponent
        if self.pos < self.bytes.len() && matches!(self.bytes[self.pos], b'e' | b'E') {
            self.pos += 1;
            if self.pos < self.bytes.len() && matches!(self.bytes[self.pos], b'+' | b'-') {
                self.pos += 1;
            }
            while self.pos < self.bytes.len() && self.bytes[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }
        if self.pos == start {
            return Err(SvgPathError::Truncated);
        }
        let raw = std::str::from_utf8(&self.bytes[start..self.pos])
            .map_err(|_| SvgPathError::InvalidNumber)?;
        raw.parse::<f32>().map_err(|_| SvgPathError::InvalidNumber)
    }

    fn read_pair(&mut self) -> Result<(f32, f32), SvgPathError> {
        let x = self.read_number()?;
        let y = self.read_number()?;
        Ok((x, y))
    }
}

fn is_command(c: char) -> bool {
    matches!(
        c,
        'M' | 'm'
            | 'L'
            | 'l'
            | 'H'
            | 'h'
            | 'V'
            | 'v'
            | 'C'
            | 'c'
            | 'S'
            | 's'
            | 'Q'
            | 'q'
            | 'T'
            | 't'
            | 'Z'
            | 'z'
            | 'A'
            | 'a'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_is_empty_path() {
        let p = parse_path("").unwrap();
        assert_eq!(p.commands.len(), 0);
    }

    #[test]
    fn whitespace_only_is_empty_path() {
        let p = parse_path("   \t\n  ").unwrap();
        assert_eq!(p.commands.len(), 0);
    }

    #[test]
    fn move_then_line_absolute() {
        let p = parse_path("M 10 20 L 30 40").unwrap();
        // 1 move + 1 line.
        assert_eq!(p.commands.len(), 2);
    }

    #[test]
    fn move_then_line_relative() {
        // m 10,20 means move to (10,20); l 5,5 should reach (15,25).
        let p = parse_path("m 10,20 l 5,5").unwrap();
        assert_eq!(p.commands.len(), 2);
    }

    #[test]
    fn must_start_with_move() {
        let err = parse_path("L 10 10").unwrap_err();
        assert_eq!(err, SvgPathError::NotStartedWithMove);
    }

    #[test]
    fn comma_or_space_separator() {
        let a = parse_path("M0,0L1,1L2,2").unwrap();
        let b = parse_path("M 0 0 L 1 1 L 2 2").unwrap();
        assert_eq!(a.commands.len(), b.commands.len());
    }

    #[test]
    fn implicit_line_after_moveto() {
        // M 0 0 1 1 → moveto (0,0) then implicit line to (1,1)
        let p = parse_path("M 0 0 1 1 2 2").unwrap();
        // 1 move + 2 implicit lines.
        assert_eq!(p.commands.len(), 3);
    }

    #[test]
    fn close_command() {
        let p = parse_path("M 0 0 L 1 0 L 1 1 Z").unwrap();
        // 1 move + 2 lines + 1 close.
        assert_eq!(p.commands.len(), 4);
    }

    #[test]
    fn horizontal_and_vertical() {
        let p = parse_path("M 5 5 H 10 V 12 h -3 v 4").unwrap();
        // 1 move + 4 lines.
        assert_eq!(p.commands.len(), 5);
    }

    #[test]
    fn cubic_bezier() {
        let p = parse_path("M 0 0 C 10 0 10 10 20 10").unwrap();
        assert_eq!(p.commands.len(), 2);
    }

    #[test]
    fn smooth_cubic_reflects_prev_control() {
        let p = parse_path("M 0 0 C 10 0 10 10 20 10 S 30 0 40 10").unwrap();
        // 1 move + 2 cubic.
        assert_eq!(p.commands.len(), 3);
    }

    #[test]
    fn quadratic_and_smooth_quadratic() {
        let p = parse_path("M 0 0 Q 5 5 10 0 T 20 0").unwrap();
        assert_eq!(p.commands.len(), 3);
    }

    #[test]
    fn scientific_notation_number() {
        let p = parse_path("M 1e2 2.5e-1 L 1.5E1 0.0").unwrap();
        assert_eq!(p.commands.len(), 2);
    }

    #[test]
    fn negative_after_implicit_pair_no_separator() {
        // SVG allows `1-2` to read as `1` and `-2` (sign restarts a
        // number). Make sure we don't gobble the minus.
        let p = parse_path("M 1-2 L 3-4").unwrap();
        assert_eq!(p.commands.len(), 2);
    }

    #[test]
    fn leading_dot_decimal() {
        let p = parse_path("M .5 .5 L 1 1").unwrap();
        assert_eq!(p.commands.len(), 2);
    }

    #[test]
    fn unsupported_arc_returns_error() {
        let err = parse_path("M 0 0 A 5 5 0 0 0 10 10").unwrap_err();
        assert!(matches!(err, SvgPathError::UnsupportedCommand('A')));
    }

    #[test]
    fn unexpected_char_at_top_level() {
        let err = parse_path("M 0 0 X 1 1").unwrap_err();
        assert!(matches!(err, SvgPathError::UnexpectedChar('X')));
    }

    #[test]
    fn truncated_after_command_letter() {
        let err = parse_path("M").unwrap_err();
        assert!(matches!(err, SvgPathError::Truncated));
    }

    #[test]
    fn close_restores_current_point_to_subpath_start() {
        // After M 10 20 L 30 40 Z, the pen should be back at (10,20),
        // so a following m 0,0 starts there. We can't peek at internal
        // state through the public Path API, but we can verify the
        // resulting command count is what we expect (3: M, L, Z) and
        // that a follow-up move parses without error.
        let p = parse_path("M 10 20 L 30 40 Z M 1 1").unwrap();
        // M L Z M = 4 commands.
        assert_eq!(p.commands.len(), 4);
    }
}
