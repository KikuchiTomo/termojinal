//! Image display protocol support for termojinal.
//!
//! Implements three terminal image protocols:
//! - **Kitty Graphics Protocol** — APC-based, supports direct transmission (PNG/RGB/RGBA),
//!   chunked transfer, placement, and deletion.
//! - **iTerm2 Inline Images** — OSC 1337-based, supports PNG/JPEG with base64 encoding.
//! - **Sixel Graphics** — DCS-based legacy format encoding 6 vertical pixels per character.
//!
//! Decoded images are stored in an `ImageStore` and referenced by the renderer for
//! GPU texture upload and display.

use base64::Engine as _;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Core types
// ---------------------------------------------------------------------------

/// A decoded terminal image (RGBA pixel data).
#[derive(Debug, Clone)]
pub struct TerminalImage {
    /// Unique image ID (assigned by protocol or auto-generated).
    pub id: u32,
    /// RGBA pixel data (4 bytes per pixel).
    pub data: Vec<u8>,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
}

/// Where an image is placed on the terminal grid.
#[derive(Debug, Clone)]
pub struct ImagePlacement {
    /// Image ID this placement refers to.
    pub image_id: u32,
    /// Column position (0-based).
    pub col: usize,
    /// Row position (signed: negative means partially scrolled off the top).
    pub row: isize,
    /// How many cell columns the image spans.
    pub cell_cols: usize,
    /// How many cell rows the image spans.
    pub cell_rows: usize,
    /// Source image width in pixels (for scaling).
    pub src_width: u32,
    /// Source image height in pixels (for scaling).
    pub src_height: u32,
}

/// Central store for decoded images and their placements.
pub struct ImageStore {
    images: HashMap<u32, TerminalImage>,
    next_id: u32,
    /// Active placements on the current screen.
    placements: Vec<ImagePlacement>,
    /// Cell dimensions (set by the application; used to compute cell_cols/cell_rows).
    cell_width_px: u32,
    cell_height_px: u32,
    /// Dirty flag: set when images/placements change and the renderer needs to update.
    dirty: bool,
}

impl ImageStore {
    pub fn new() -> Self {
        Self {
            images: HashMap::new(),
            next_id: 1,
            placements: Vec::new(),
            cell_width_px: 8,
            cell_height_px: 16,
            dirty: false,
        }
    }

    /// Set the cell dimensions (in pixels) used for computing placement cell spans.
    pub fn set_cell_size(&mut self, width: u32, height: u32) {
        if width > 0 {
            self.cell_width_px = width;
        }
        if height > 0 {
            self.cell_height_px = height;
        }
    }

    /// Allocate the next auto-generated image ID.
    pub fn next_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1).max(1);
        id
    }

    /// Store a decoded image. If an image with the same ID already exists, it is replaced.
    pub fn store_image(&mut self, image: TerminalImage) {
        self.images.insert(image.id, image);
        self.dirty = true;
    }

    /// Get an image by ID.
    pub fn get_image(&self, id: u32) -> Option<&TerminalImage> {
        self.images.get(&id)
    }

    /// Add a placement. Computes cell_cols/cell_rows from pixel dimensions if not provided.
    pub fn add_placement(&mut self, mut placement: ImagePlacement) {
        if placement.cell_cols == 0 && placement.src_width > 0 {
            placement.cell_cols =
                (placement.src_width as usize + self.cell_width_px as usize - 1)
                    / self.cell_width_px as usize;
        }
        if placement.cell_rows == 0 && placement.src_height > 0 {
            placement.cell_rows =
                (placement.src_height as usize + self.cell_height_px as usize - 1)
                    / self.cell_height_px as usize;
        }
        // Ensure at least 1x1 cell.
        placement.cell_cols = placement.cell_cols.max(1);
        placement.cell_rows = placement.cell_rows.max(1);
        self.placements.push(placement);
        self.dirty = true;
    }

    /// Delete an image and all its placements by ID.
    pub fn delete_image(&mut self, id: u32) {
        self.images.remove(&id);
        self.placements.retain(|p| p.image_id != id);
        self.dirty = true;
    }

    /// Delete all images and placements.
    pub fn delete_all(&mut self) {
        self.images.clear();
        self.placements.clear();
        self.dirty = true;
    }

    /// Get all current placements.
    pub fn placements(&self) -> &[ImagePlacement] {
        &self.placements
    }

    /// Get all stored images.
    pub fn images(&self) -> &HashMap<u32, TerminalImage> {
        &self.images
    }

    /// Check and clear the dirty flag.
    pub fn take_dirty(&mut self) -> bool {
        let was_dirty = self.dirty;
        self.dirty = false;
        was_dirty
    }

    /// Check if there are any placements.
    pub fn has_placements(&self) -> bool {
        !self.placements.is_empty()
    }

    /// Adjust all image placements when the terminal scrolls up by `lines` rows.
    ///
    /// Shifts all placement rows up and removes images that have fully scrolled
    /// off the top of the screen.  Also garbage-collects images that no longer
    /// have any placements.
    pub fn scroll_up(&mut self, lines: usize) {
        if lines == 0 || self.placements.is_empty() {
            return;
        }
        let lines = lines as isize;
        // Shift all placement rows up (row can go negative = scrolled off top).
        for p in &mut self.placements {
            p.row -= lines;
        }
        // Remove placements whose bottom edge is above the screen top.
        self.placements.retain(|p| p.row + p.cell_rows as isize > 0);
        // Garbage-collect images that no longer have any placements.
        let placed_ids: Vec<u32> = self.placements.iter().map(|p| p.image_id).collect();
        self.images.retain(|id, _| placed_ids.contains(id));
        self.dirty = true;
    }

    /// Limit image placement size to fit within the visible terminal grid.
    ///
    /// Called after computing cell_cols/cell_rows so images don't extend
    /// beyond the terminal dimensions.
    pub fn cap_placement_size(&mut self, max_cols: usize, max_rows: usize) {
        if let Some(p) = self.placements.last_mut() {
            if p.cell_cols > max_cols {
                p.cell_cols = max_cols;
            }
            if p.cell_rows > max_rows {
                p.cell_rows = max_rows;
            }
        }
    }
}

impl Default for ImageStore {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Kitty Graphics Protocol
// ---------------------------------------------------------------------------

/// Kitty Graphics action type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KittyAction {
    /// `a=t` — Transmit image data (store but don't display).
    Transmit,
    /// `a=T` — Transmit and display at cursor.
    TransmitAndDisplay,
    /// `a=p` — Display a previously transmitted image.
    Place,
    /// `a=d` — Delete image(s).
    Delete,
    /// `a=q` — Query support.
    Query,
}

/// Kitty Graphics data format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KittyFormat {
    /// `f=24` — RGB (3 bytes per pixel).
    Rgb,
    /// `f=32` — RGBA (4 bytes per pixel).
    Rgba,
    /// `f=100` — PNG compressed.
    Png,
}

/// Parsed Kitty Graphics command header.
#[derive(Debug, Clone)]
pub struct KittyCommand {
    pub action: KittyAction,
    pub format: KittyFormat,
    /// Image ID (`i=`).
    pub image_id: Option<u32>,
    /// Pixel width (`s=`).
    pub width: Option<u32>,
    /// Pixel height (`v=`).
    pub height: Option<u32>,
    /// Cell columns for placement (`c=`).
    pub cell_cols: Option<usize>,
    /// Cell rows for placement (`r=`).
    pub cell_rows: Option<usize>,
    /// More data chunks follow (`m=1`).
    pub more_chunks: bool,
    /// Transmission type (only `d` = direct is supported).
    pub transmission: char,
    /// Delete target for `a=d`.
    pub delete_target: Option<char>,
    /// Quiet mode (`q=`). 0 = normal, 1 = suppress OK, 2 = suppress all.
    pub quiet: u8,
}

impl Default for KittyCommand {
    fn default() -> Self {
        Self {
            action: KittyAction::TransmitAndDisplay,
            format: KittyFormat::Rgba,
            image_id: None,
            width: None,
            height: None,
            cell_cols: None,
            cell_rows: None,
            more_chunks: false,
            transmission: 'd',
            delete_target: None,
            quiet: 0,
        }
    }
}

/// Parse a Kitty Graphics payload header (everything before the `;`).
///
/// The header consists of comma-separated `key=value` pairs.
pub fn parse_kitty_header(header: &str) -> KittyCommand {
    let mut cmd = KittyCommand::default();

    for kv in header.split(',') {
        let mut parts = kv.splitn(2, '=');
        let key = match parts.next() {
            Some(k) => k.trim(),
            None => continue,
        };
        let value = parts.next().unwrap_or("").trim();

        match key {
            "a" => {
                cmd.action = match value {
                    "t" => KittyAction::Transmit,
                    "T" => KittyAction::TransmitAndDisplay,
                    "p" => KittyAction::Place,
                    "d" => KittyAction::Delete,
                    "q" => KittyAction::Query,
                    _ => KittyAction::TransmitAndDisplay,
                };
            }
            "f" => {
                cmd.format = match value {
                    "24" => KittyFormat::Rgb,
                    "32" => KittyFormat::Rgba,
                    "100" => KittyFormat::Png,
                    _ => KittyFormat::Rgba,
                };
            }
            "t" => {
                cmd.transmission = value.chars().next().unwrap_or('d');
            }
            "i" => {
                cmd.image_id = value.parse().ok();
            }
            "s" => {
                cmd.width = value.parse().ok();
            }
            "v" => {
                cmd.height = value.parse().ok();
            }
            "c" => {
                cmd.cell_cols = value.parse().ok();
            }
            "r" => {
                cmd.cell_rows = value.parse().ok();
            }
            "m" => {
                cmd.more_chunks = value == "1";
            }
            "d" => {
                cmd.delete_target = value.chars().next();
            }
            "q" => {
                cmd.quiet = value.parse().unwrap_or(0);
            }
            _ => {
                log::trace!("kitty graphics: unknown key '{key}'");
            }
        }
    }

    cmd
}

/// Accumulator for chunked Kitty Graphics transmissions.
#[derive(Debug, Default)]
pub struct KittyAccumulator {
    /// The command header from the first chunk.
    pub command: Option<KittyCommand>,
    /// Accumulated base64-encoded data across chunks.
    pub base64_data: String,
}

impl KittyAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a parsed APC payload. Returns `Some(KittyCommand, decoded_bytes)` when
    /// the final chunk arrives (`m=0`).
    pub fn feed(
        &mut self,
        header: &str,
        base64_chunk: &str,
    ) -> Option<(KittyCommand, Vec<u8>)> {
        let cmd = parse_kitty_header(header);

        if self.command.is_none() {
            self.command = Some(cmd.clone());
        }

        self.base64_data.push_str(base64_chunk);

        if cmd.more_chunks {
            // Update more_chunks on the stored command.
            if let Some(ref mut stored) = self.command {
                stored.more_chunks = true;
            }
            return None;
        }

        // Final chunk: decode all accumulated data.
        let full_cmd = self.command.take().unwrap_or(cmd);
        let b64 = std::mem::take(&mut self.base64_data);

        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&b64)
            .unwrap_or_default();

        Some((full_cmd, decoded))
    }

    /// Reset the accumulator (e.g., on error).
    pub fn reset(&mut self) {
        self.command = None;
        self.base64_data.clear();
    }
}

/// Process a complete Kitty Graphics command with decoded payload.
///
/// `cursor_col` and `cursor_row` are the current cursor position for placement.
pub fn process_kitty_command(
    cmd: &KittyCommand,
    payload: &[u8],
    store: &mut ImageStore,
    cursor_col: usize,
    cursor_row: usize,
) {
    match cmd.action {
        KittyAction::Transmit | KittyAction::TransmitAndDisplay => {
            // Only support direct transmission (t=d).
            if cmd.transmission != 'd' {
                log::trace!(
                    "kitty graphics: unsupported transmission type '{}'",
                    cmd.transmission
                );
                return;
            }

            let rgba_data = match cmd.format {
                KittyFormat::Png => match decode_png(payload) {
                    Some(img) => img,
                    None => {
                        log::trace!("kitty graphics: failed to decode PNG");
                        return;
                    }
                },
                KittyFormat::Rgba => {
                    let w = cmd.width.unwrap_or(0);
                    let h = cmd.height.unwrap_or(0);
                    if w == 0 || h == 0 {
                        log::trace!("kitty graphics: RGBA format requires s= and v=");
                        return;
                    }
                    let expected = (w * h * 4) as usize;
                    if payload.len() != expected {
                        log::trace!(
                            "kitty graphics: RGBA size mismatch: expected {expected}, got {}",
                            payload.len()
                        );
                        return;
                    }
                    DecodedImage {
                        data: payload.to_vec(),
                        width: w,
                        height: h,
                    }
                }
                KittyFormat::Rgb => {
                    let w = cmd.width.unwrap_or(0);
                    let h = cmd.height.unwrap_or(0);
                    if w == 0 || h == 0 {
                        log::trace!("kitty graphics: RGB format requires s= and v=");
                        return;
                    }
                    let expected = (w * h * 3) as usize;
                    if payload.len() != expected {
                        log::trace!(
                            "kitty graphics: RGB size mismatch: expected {expected}, got {}",
                            payload.len()
                        );
                        return;
                    }
                    // Convert RGB to RGBA.
                    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
                    for chunk in payload.chunks_exact(3) {
                        rgba.extend_from_slice(chunk);
                        rgba.push(255);
                    }
                    DecodedImage {
                        data: rgba,
                        width: w,
                        height: h,
                    }
                }
            };

            let id = cmd.image_id.unwrap_or_else(|| store.next_id());

            let image = TerminalImage {
                id,
                data: rgba_data.data,
                width: rgba_data.width,
                height: rgba_data.height,
            };

            let should_place = cmd.action == KittyAction::TransmitAndDisplay;
            let img_w = image.width;
            let img_h = image.height;
            store.store_image(image);

            if should_place {
                store.add_placement(ImagePlacement {
                    image_id: id,
                    col: cursor_col,
                    row: cursor_row as isize,
                    cell_cols: cmd.cell_cols.unwrap_or(0),
                    cell_rows: cmd.cell_rows.unwrap_or(0),
                    src_width: img_w,
                    src_height: img_h,
                });
            }

            log::debug!(
                "kitty graphics: stored image id={id} {}x{} ({})",
                img_w,
                img_h,
                if should_place { "placed" } else { "transmit only" }
            );
        }
        KittyAction::Place => {
            let id = match cmd.image_id {
                Some(id) => id,
                None => {
                    log::trace!("kitty graphics: place requires i=");
                    return;
                }
            };
            let image = match store.get_image(id) {
                Some(img) => img,
                None => {
                    log::trace!("kitty graphics: image id={id} not found");
                    return;
                }
            };
            let img_w = image.width;
            let img_h = image.height;
            store.add_placement(ImagePlacement {
                image_id: id,
                col: cursor_col,
                row: cursor_row as isize,
                cell_cols: cmd.cell_cols.unwrap_or(0),
                cell_rows: cmd.cell_rows.unwrap_or(0),
                src_width: img_w,
                src_height: img_h,
            });
            log::debug!("kitty graphics: placed image id={id} at ({cursor_col}, {cursor_row})");
        }
        KittyAction::Delete => {
            match cmd.delete_target {
                Some('a') | None => {
                    // Delete all images.
                    if let Some(id) = cmd.image_id {
                        store.delete_image(id);
                        log::debug!("kitty graphics: deleted image id={id}");
                    } else {
                        store.delete_all();
                        log::debug!("kitty graphics: deleted all images");
                    }
                }
                Some('i') => {
                    if let Some(id) = cmd.image_id {
                        store.delete_image(id);
                        log::debug!("kitty graphics: deleted image id={id}");
                    }
                }
                Some(target) => {
                    log::trace!("kitty graphics: unsupported delete target '{target}'");
                }
            }
        }
        KittyAction::Query => {
            log::trace!("kitty graphics: query (not sending response yet)");
        }
    }
}

// ---------------------------------------------------------------------------
// iTerm2 Inline Images (OSC 1337)
// ---------------------------------------------------------------------------

/// Parsed iTerm2 image metadata from the initial `MultipartFile=` or `File=` header.
#[derive(Debug, Clone, Default)]
struct Iterm2Params {
    inline: bool,
    width: Option<String>,
    height: Option<String>,
    preserve_aspect: bool,
}

/// Accumulator for iTerm2 multipart image transfer.
///
/// The multipart protocol works as follows:
///   1. `OSC 1337 ; MultipartFile=<params> ST` — begin transfer (metadata, no pixel data)
///   2. `OSC 1337 ; FilePart=<base64chunk> ST` — one or more data chunks
///   3. `OSC 1337 ; FileEnd ST`                — finalize: decode & place the image
#[derive(Debug, Default)]
pub struct Iterm2Accumulator {
    /// Parsed parameters from the initial MultipartFile header.
    params: Option<Iterm2Params>,
    /// Accumulated base64-encoded data across FilePart chunks.
    base64_data: String,
}

impl Iterm2Accumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Begin a new multipart transfer. Parses the `MultipartFile=<params>` payload.
    pub fn begin(&mut self, payload: &str) {
        let rest = match payload.strip_prefix("MultipartFile=") {
            Some(r) => r,
            None => {
                log::trace!("iterm2 multipart: missing 'MultipartFile=' prefix");
                return;
            }
        };

        let params = parse_iterm2_params(rest);
        self.params = Some(params);
        self.base64_data.clear();
        log::debug!("iterm2 multipart: begin");
    }

    /// Accumulate a `FilePart=<base64chunk>` chunk.
    pub fn add_part(&mut self, payload: &str) {
        let chunk = match payload.strip_prefix("FilePart=") {
            Some(c) => c,
            None => {
                log::trace!("iterm2 multipart: missing 'FilePart=' prefix");
                return;
            }
        };
        self.base64_data.push_str(chunk);
    }

    /// Finalize on `FileEnd`: decode the accumulated data and place the image.
    /// Returns `true` if an image was successfully placed.
    pub fn finish(
        &mut self,
        store: &mut ImageStore,
        cursor_col: usize,
        cursor_row: usize,
    ) -> bool {
        let params = match self.params.take() {
            Some(p) => p,
            None => {
                log::trace!("iterm2 multipart: FileEnd without prior MultipartFile");
                self.base64_data.clear();
                return false;
            }
        };

        if !params.inline {
            log::trace!("iterm2 multipart: inline=0, not displaying");
            self.base64_data.clear();
            return false;
        }

        let b64 = std::mem::take(&mut self.base64_data);
        let raw_bytes = match base64::engine::general_purpose::STANDARD.decode(&b64) {
            Ok(b) => b,
            Err(e) => {
                log::trace!("iterm2 multipart: base64 decode error: {e}");
                return false;
            }
        };

        let decoded = if is_png(&raw_bytes) {
            decode_png(&raw_bytes)
        } else if is_jpeg(&raw_bytes) {
            decode_jpeg(&raw_bytes)
        } else {
            log::trace!("iterm2 multipart: unsupported format (not PNG or JPEG)");
            None
        };

        let decoded = match decoded {
            Some(d) => d,
            None => {
                log::trace!("iterm2 multipart: failed to decode image data");
                return false;
            }
        };

        let id = store.next_id();
        let img_w = decoded.width;
        let img_h = decoded.height;

        let cell_cols = params
            .width
            .and_then(|w| parse_iterm2_dimension(&w))
            .unwrap_or(0);
        let cell_rows = params
            .height
            .and_then(|h| parse_iterm2_dimension(&h))
            .unwrap_or(0);

        store.store_image(TerminalImage {
            id,
            data: decoded.data,
            width: img_w,
            height: img_h,
        });

        store.add_placement(ImagePlacement {
            image_id: id,
            col: cursor_col,
            row: cursor_row as isize,
            cell_cols,
            cell_rows,
            src_width: img_w,
            src_height: img_h,
        });

        log::debug!(
            "iterm2 multipart: stored and placed id={id} {img_w}x{img_h}"
        );
        true
    }

    /// Whether a multipart transfer is currently in progress.
    pub fn is_active(&self) -> bool {
        self.params.is_some()
    }
}

/// Parse iTerm2 key=value parameters from a params string (without the `File=` or
/// `MultipartFile=` prefix, and without the `:base64data` suffix if present).
fn parse_iterm2_params(params_str: &str) -> Iterm2Params {
    let mut result = Iterm2Params {
        inline: false,
        width: None,
        height: None,
        preserve_aspect: true,
    };

    for kv in params_str.split(';') {
        let mut parts = kv.splitn(2, '=');
        let key = match parts.next() {
            Some(k) => k,
            None => continue,
        };
        let value = parts.next().unwrap_or("");
        match key {
            "inline" => result.inline = value == "1",
            "width" => result.width = Some(value.to_string()),
            "height" => result.height = Some(value.to_string()),
            "preserveAspectRatio" => result.preserve_aspect = value != "0",
            _ => {}
        }
    }

    result
}

/// Parse an iTerm2 inline image OSC 1337 payload.
///
/// Format: `File=<params>:<base64data>`
/// Params are semicolon-separated `key=value` pairs.
pub fn parse_iterm2_image(
    payload: &str,
    store: &mut ImageStore,
    cursor_col: usize,
    cursor_row: usize,
) {
    // The payload after "File=" is: params:base64data
    let rest = match payload.strip_prefix("File=") {
        Some(r) => r,
        None => {
            log::trace!("iterm2 image: missing 'File=' prefix");
            return;
        }
    };

    let (params_str, b64_data) = match rest.rfind(':') {
        Some(idx) => (&rest[..idx], &rest[idx + 1..]),
        None => {
            log::trace!("iterm2 image: missing ':' separator");
            return;
        }
    };

    // Parse params.
    let mut inline = false;
    let mut _name = String::new();
    let mut _size: Option<usize> = None;
    let mut width_param: Option<String> = None;
    let mut height_param: Option<String> = None;
    let mut _preserve_aspect = true;

    for kv in params_str.split(';') {
        let mut parts = kv.splitn(2, '=');
        let key = match parts.next() {
            Some(k) => k,
            None => continue,
        };
        let value = parts.next().unwrap_or("");
        match key {
            "inline" => inline = value == "1",
            "name" => {
                _name = base64::engine::general_purpose::STANDARD
                    .decode(value)
                    .ok()
                    .and_then(|b| String::from_utf8(b).ok())
                    .unwrap_or_default();
            }
            "size" => _size = value.parse().ok(),
            "width" => width_param = Some(value.to_string()),
            "height" => height_param = Some(value.to_string()),
            "preserveAspectRatio" => _preserve_aspect = value != "0",
            _ => {}
        }
    }

    if !inline {
        log::trace!("iterm2 image: inline=0, not displaying");
        return;
    }

    // Decode base64 data.
    let raw_bytes = match base64::engine::general_purpose::STANDARD.decode(b64_data) {
        Ok(b) => b,
        Err(e) => {
            log::trace!("iterm2 image: base64 decode error: {e}");
            return;
        }
    };

    // Detect format and decode to RGBA.
    let decoded = if is_png(&raw_bytes) {
        decode_png(&raw_bytes)
    } else if is_jpeg(&raw_bytes) {
        decode_jpeg(&raw_bytes)
    } else {
        log::trace!("iterm2 image: unsupported format (not PNG or JPEG)");
        None
    };

    let decoded = match decoded {
        Some(d) => d,
        None => {
            log::trace!("iterm2 image: failed to decode image data");
            return;
        }
    };

    let id = store.next_id();
    let img_w = decoded.width;
    let img_h = decoded.height;

    // Parse width/height params for cell sizing.
    let cell_cols = width_param
        .and_then(|w| parse_iterm2_dimension(&w))
        .unwrap_or(0);
    let cell_rows = height_param
        .and_then(|h| parse_iterm2_dimension(&h))
        .unwrap_or(0);

    store.store_image(TerminalImage {
        id,
        data: decoded.data,
        width: img_w,
        height: img_h,
    });

    store.add_placement(ImagePlacement {
        image_id: id,
        col: cursor_col,
        row: cursor_row as isize,
        cell_cols,
        cell_rows,
        src_width: img_w,
        src_height: img_h,
    });

    log::debug!("iterm2 image: stored and placed id={id} {img_w}x{img_h}");
}

/// Parse an iTerm2 dimension string (e.g., "80px", "10", "auto").
/// Returns cell count or 0 for auto-sizing.
fn parse_iterm2_dimension(s: &str) -> Option<usize> {
    if s == "auto" || s.is_empty() {
        return Some(0);
    }
    if let Some(px) = s.strip_suffix("px") {
        // Pixel dimension — we'd need cell size to convert.
        // Return 0 to let add_placement compute from src dimensions.
        let _px_val: u32 = px.parse().ok()?;
        Some(0)
    } else {
        // Treat as cell count.
        s.parse().ok()
    }
}

// ---------------------------------------------------------------------------
// Sixel Graphics
// ---------------------------------------------------------------------------

/// Sixel color register.
#[derive(Debug, Clone, Copy)]
struct SixelColor {
    r: u8,
    g: u8,
    b: u8,
}

/// Decode sixel data into an RGBA pixel buffer.
///
/// Sixel format: each character in the range `?` (0x3F) to `~` (0x7E)
/// encodes 6 vertical pixels. Color is set via `#<register>` commands.
/// `$` returns to the beginning of the current row of sixels.
/// `-` moves to the next row of sixels (6 pixels down).
/// `!<count><char>` is a repeat introducer.
pub fn decode_sixel(data: &[u8]) -> Option<DecodedImage> {
    // Default palette: start with VGA 16 colors.
    let mut palette: HashMap<u16, SixelColor> = HashMap::new();
    init_default_sixel_palette(&mut palette);

    let mut current_color: u16 = 0;
    let mut x: u32 = 0;
    let mut y: u32 = 0;
    let mut max_x: u32 = 0;
    let mut max_y: u32 = 0;

    // First pass: determine dimensions.
    {
        let mut px = 0u32;
        let mut py = 0u32;
        let mut i = 0;
        while i < data.len() {
            let b = data[i];
            match b {
                b'$' => {
                    px = 0;
                }
                b'-' => {
                    px = 0;
                    py += 6;
                }
                b'#' => {
                    // Skip color command.
                    i += 1;
                    while i < data.len() && (data[i].is_ascii_digit() || data[i] == b';') {
                        i += 1;
                    }
                    continue;
                }
                b'!' => {
                    // Repeat: !<count><char>
                    i += 1;
                    let mut count = 0u32;
                    while i < data.len() && data[i].is_ascii_digit() {
                        count = count * 10 + (data[i] - b'0') as u32;
                        i += 1;
                    }
                    if i < data.len() && data[i] >= 0x3F && data[i] <= 0x7E {
                        px += count;
                        if px > max_x {
                            max_x = px;
                        }
                        if py + 6 > max_y {
                            max_y = py + 6;
                        }
                    }
                    i += 1;
                    continue;
                }
                0x3F..=0x7E => {
                    px += 1;
                    if px > max_x {
                        max_x = px;
                    }
                    if py + 6 > max_y {
                        max_y = py + 6;
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    if max_x == 0 || max_y == 0 {
        return None;
    }

    let width = max_x;
    let height = max_y;
    let mut pixels = vec![0u8; (width * height * 4) as usize];

    // Second pass: draw pixels.
    let mut i = 0;
    while i < data.len() {
        let b = data[i];
        match b {
            b'$' => {
                x = 0;
            }
            b'-' => {
                x = 0;
                y += 6;
            }
            b'#' => {
                // Color command: #<register> or #<register>;<type>;<p1>;<p2>;<p3>
                i += 1;
                let mut reg = 0u16;
                while i < data.len() && data[i].is_ascii_digit() {
                    reg = reg * 10 + (data[i] - b'0') as u16;
                    i += 1;
                }
                if i < data.len() && data[i] == b';' {
                    // Color definition.
                    i += 1;
                    let mut params = Vec::new();
                    let mut num = 0u16;
                    let mut has_num = false;
                    while i < data.len() && (data[i].is_ascii_digit() || data[i] == b';') {
                        if data[i] == b';' {
                            params.push(num);
                            num = 0;
                            has_num = false;
                        } else {
                            num = num * 10 + (data[i] - b'0') as u16;
                            has_num = true;
                        }
                        i += 1;
                    }
                    if has_num {
                        params.push(num);
                    }
                    if params.len() >= 4 {
                        let color_type = params[0];
                        let (r, g, b_val) = if color_type == 2 {
                            // RGB percentages (0-100).
                            let rp = params.get(1).copied().unwrap_or(0).min(100);
                            let gp = params.get(2).copied().unwrap_or(0).min(100);
                            let bp = params.get(3).copied().unwrap_or(0).min(100);
                            (
                                (rp as u32 * 255 / 100) as u8,
                                (gp as u32 * 255 / 100) as u8,
                                (bp as u32 * 255 / 100) as u8,
                            )
                        } else if color_type == 1 {
                            // HLS (Hue/Lightness/Saturation).
                            let h = params.get(1).copied().unwrap_or(0);
                            let l = params.get(2).copied().unwrap_or(0);
                            let s = params.get(3).copied().unwrap_or(0);
                            hls_to_rgb(h, l, s)
                        } else {
                            (0, 0, 0)
                        };
                        palette.insert(reg, SixelColor { r, g, b: b_val });
                    }
                }
                current_color = reg;
                continue;
            }
            b'!' => {
                // Repeat: !<count><char>
                i += 1;
                let mut count = 0u32;
                while i < data.len() && data[i].is_ascii_digit() {
                    count = count * 10 + (data[i] - b'0') as u32;
                    i += 1;
                }
                if i < data.len() && data[i] >= 0x3F && data[i] <= 0x7E {
                    let sixel_val = data[i] - 0x3F;
                    let color = palette
                        .get(&current_color)
                        .copied()
                        .unwrap_or(SixelColor { r: 255, g: 255, b: 255 });
                    for rep in 0..count {
                        draw_sixel_column(
                            &mut pixels,
                            width,
                            height,
                            x + rep,
                            y,
                            sixel_val,
                            &color,
                        );
                    }
                    x += count;
                }
                i += 1;
                continue;
            }
            0x3F..=0x7E => {
                let sixel_val = b - 0x3F;
                let color = palette
                    .get(&current_color)
                    .copied()
                    .unwrap_or(SixelColor { r: 255, g: 255, b: 255 });
                draw_sixel_column(&mut pixels, width, height, x, y, sixel_val, &color);
                x += 1;
            }
            _ => {}
        }
        i += 1;
    }

    Some(DecodedImage {
        data: pixels,
        width,
        height,
    })
}

/// Draw a single sixel column (6 vertical pixels) into the pixel buffer.
fn draw_sixel_column(
    pixels: &mut [u8],
    width: u32,
    height: u32,
    x: u32,
    y: u32,
    sixel_val: u8,
    color: &SixelColor,
) {
    for bit in 0..6u32 {
        if sixel_val & (1 << bit) != 0 {
            let py = y + bit;
            if x < width && py < height {
                let offset = ((py * width + x) * 4) as usize;
                if offset + 3 < pixels.len() {
                    pixels[offset] = color.r;
                    pixels[offset + 1] = color.g;
                    pixels[offset + 2] = color.b;
                    pixels[offset + 3] = 255;
                }
            }
        }
    }
}

/// Initialize the default VGA 16-color sixel palette.
fn init_default_sixel_palette(palette: &mut HashMap<u16, SixelColor>) {
    let vga16: [(u8, u8, u8); 16] = [
        (0, 0, 0),       // 0: black
        (0, 0, 170),     // 1: blue
        (170, 0, 0),     // 2: red
        (0, 170, 0),     // 3: green
        (170, 0, 170),   // 4: magenta
        (0, 170, 170),   // 5: cyan
        (170, 170, 0),   // 6: yellow
        (170, 170, 170), // 7: white
        (85, 85, 85),    // 8: bright black
        (85, 85, 255),   // 9: bright blue
        (255, 85, 85),   // 10: bright red
        (85, 255, 85),   // 11: bright green
        (255, 85, 255),  // 12: bright magenta
        (85, 255, 255),  // 13: bright cyan
        (255, 255, 85),  // 14: bright yellow
        (255, 255, 255), // 15: bright white
    ];
    for (i, (r, g, b)) in vga16.iter().enumerate() {
        palette.insert(i as u16, SixelColor { r: *r, g: *g, b: *b });
    }
}

/// Convert HLS (Hue 0-360, Lightness 0-100, Saturation 0-100) to RGB.
fn hls_to_rgb(h: u16, l: u16, s: u16) -> (u8, u8, u8) {
    let h = (h % 360) as f64;
    let l = l.min(100) as f64 / 100.0;
    let s = s.min(100) as f64 / 100.0;

    if s == 0.0 {
        let v = (l * 255.0) as u8;
        return (v, v, v);
    }

    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let hk = h / 360.0;

    let to_rgb = |t: f64| -> u8 {
        let t = if t < 0.0 {
            t + 1.0
        } else if t > 1.0 {
            t - 1.0
        } else {
            t
        };
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (v * 255.0).round().min(255.0).max(0.0) as u8
    };

    (
        to_rgb(hk + 1.0 / 3.0),
        to_rgb(hk),
        to_rgb(hk - 1.0 / 3.0),
    )
}

/// Process a complete Sixel DCS sequence.
///
/// The `data` should be the sixel data portion (after the `q` introducer).
pub fn process_sixel(
    data: &[u8],
    store: &mut ImageStore,
    cursor_col: usize,
    cursor_row: usize,
) {
    match decode_sixel(data) {
        Some(decoded) => {
            let id = store.next_id();
            let w = decoded.width;
            let h = decoded.height;
            store.store_image(TerminalImage {
                id,
                data: decoded.data,
                width: w,
                height: h,
            });
            store.add_placement(ImagePlacement {
                image_id: id,
                col: cursor_col,
                row: cursor_row as isize,
                cell_cols: 0,
                cell_rows: 0,
                src_width: w,
                src_height: h,
            });
            log::debug!("sixel: decoded and placed id={id} {w}x{h}");
        }
        None => {
            log::trace!("sixel: failed to decode data");
        }
    }
}

// ---------------------------------------------------------------------------
// Image format detection and decoding
// ---------------------------------------------------------------------------

/// Intermediate decoded image before storing.
pub struct DecodedImage {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// Check if data starts with PNG magic bytes.
fn is_png(data: &[u8]) -> bool {
    data.len() >= 8 && data[..8] == [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]
}

/// Check if data starts with JPEG magic bytes.
fn is_jpeg(data: &[u8]) -> bool {
    data.len() >= 2 && data[0] == 0xFF && data[1] == 0xD8
}

/// Decode PNG data to RGBA pixels using the `image` crate.
fn decode_png(data: &[u8]) -> Option<DecodedImage> {
    let img = image::load_from_memory_with_format(data, image::ImageFormat::Png).ok()?;
    let rgba = img.to_rgba8();
    Some(DecodedImage {
        width: rgba.width(),
        height: rgba.height(),
        data: rgba.into_raw(),
    })
}

/// Decode JPEG data to RGBA pixels using the `image` crate.
fn decode_jpeg(data: &[u8]) -> Option<DecodedImage> {
    let img = image::load_from_memory_with_format(data, image::ImageFormat::Jpeg).ok()?;
    let rgba = img.to_rgba8();
    Some(DecodedImage {
        width: rgba.width(),
        height: rgba.height(),
        data: rgba.into_raw(),
    })
}

// ---------------------------------------------------------------------------
// APC sequence extraction (pre-processor for vte)
// ---------------------------------------------------------------------------

/// State for extracting APC sequences from a byte stream before vte processing.
///
/// The vte crate (0.13) does not dispatch APC content — it enters
/// `SosPmApcString` and ignores all bytes until ST. We pre-scan the stream
/// and extract APC payloads (used by Kitty Graphics Protocol) before passing
/// the remaining bytes to vte.
#[derive(Debug)]
pub struct ApcExtractor {
    state: ApcState,
    buffer: Vec<u8>,
}

#[derive(Debug, PartialEq)]
enum ApcState {
    /// Normal pass-through.
    Ground,
    /// Saw ESC (0x1B), waiting for `_` (0x5F) to start APC.
    Escape,
    /// Inside APC body, accumulating until ST.
    InApc,
    /// Inside APC body, saw ESC — waiting for `\` to end (ESC \).
    InApcEscape,
}

/// Result of processing bytes through the APC extractor.
pub struct ApcExtractResult {
    /// Bytes that should be fed to the vte parser (APC sequences stripped out).
    pub passthrough: Vec<u8>,
    /// Complete APC payloads that were extracted.
    pub apc_payloads: Vec<Vec<u8>>,
}

impl ApcExtractor {
    pub fn new() -> Self {
        Self {
            state: ApcState::Ground,
            buffer: Vec::new(),
        }
    }

    /// Process a chunk of bytes, extracting any APC sequences.
    ///
    /// Returns the bytes to pass through to vte and any complete APC payloads.
    pub fn process(&mut self, data: &[u8]) -> ApcExtractResult {
        let mut passthrough = Vec::with_capacity(data.len());
        let mut apc_payloads = Vec::new();

        for &byte in data {
            match self.state {
                ApcState::Ground => {
                    if byte == 0x1B {
                        self.state = ApcState::Escape;
                    } else {
                        passthrough.push(byte);
                    }
                }
                ApcState::Escape => {
                    if byte == b'_' {
                        // Start of APC.
                        self.state = ApcState::InApc;
                        self.buffer.clear();
                    } else {
                        // Not APC — pass the ESC and this byte through.
                        passthrough.push(0x1B);
                        passthrough.push(byte);
                        self.state = ApcState::Ground;
                    }
                }
                ApcState::InApc => {
                    if byte == 0x1B {
                        self.state = ApcState::InApcEscape;
                    } else if byte == 0x9C {
                        // ST (single byte C1 form).
                        apc_payloads.push(std::mem::take(&mut self.buffer));
                        self.state = ApcState::Ground;
                    } else {
                        self.buffer.push(byte);
                    }
                }
                ApcState::InApcEscape => {
                    if byte == b'\\' {
                        // ESC \ = ST — end of APC.
                        apc_payloads.push(std::mem::take(&mut self.buffer));
                        self.state = ApcState::Ground;
                    } else {
                        // Not ST — the ESC was part of the APC body.
                        self.buffer.push(0x1B);
                        self.buffer.push(byte);
                        self.state = ApcState::InApc;
                    }
                }
            }
        }

        ApcExtractResult {
            passthrough,
            apc_payloads,
        }
    }

    /// Reset the extractor state (e.g., on terminal reset).
    pub fn reset(&mut self) {
        self.state = ApcState::Ground;
        self.buffer.clear();
    }
}

impl Default for ApcExtractor {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- Kitty header parsing --

    #[test]
    fn test_parse_kitty_header_basic() {
        let cmd = parse_kitty_header("a=T,f=100,i=42,s=100,v=50");
        assert_eq!(cmd.action, KittyAction::TransmitAndDisplay);
        assert_eq!(cmd.format, KittyFormat::Png);
        assert_eq!(cmd.image_id, Some(42));
        assert_eq!(cmd.width, Some(100));
        assert_eq!(cmd.height, Some(50));
        assert!(!cmd.more_chunks);
    }

    #[test]
    fn test_parse_kitty_header_defaults() {
        let cmd = parse_kitty_header("");
        assert_eq!(cmd.action, KittyAction::TransmitAndDisplay);
        assert_eq!(cmd.format, KittyFormat::Rgba);
        assert_eq!(cmd.image_id, None);
    }

    #[test]
    fn test_parse_kitty_header_transmit() {
        let cmd = parse_kitty_header("a=t,f=32,i=1,s=2,v=2");
        assert_eq!(cmd.action, KittyAction::Transmit);
        assert_eq!(cmd.format, KittyFormat::Rgba);
        assert_eq!(cmd.width, Some(2));
        assert_eq!(cmd.height, Some(2));
    }

    #[test]
    fn test_parse_kitty_header_delete() {
        let cmd = parse_kitty_header("a=d,d=i,i=5");
        assert_eq!(cmd.action, KittyAction::Delete);
        assert_eq!(cmd.delete_target, Some('i'));
        assert_eq!(cmd.image_id, Some(5));
    }

    #[test]
    fn test_parse_kitty_header_chunked() {
        let cmd = parse_kitty_header("a=T,f=100,m=1");
        assert!(cmd.more_chunks);
        let cmd2 = parse_kitty_header("m=0");
        assert!(!cmd2.more_chunks);
    }

    #[test]
    fn test_parse_kitty_header_rgb() {
        let cmd = parse_kitty_header("a=T,f=24,s=4,v=4");
        assert_eq!(cmd.format, KittyFormat::Rgb);
    }

    #[test]
    fn test_parse_kitty_header_place() {
        let cmd = parse_kitty_header("a=p,i=10,c=5,r=3");
        assert_eq!(cmd.action, KittyAction::Place);
        assert_eq!(cmd.image_id, Some(10));
        assert_eq!(cmd.cell_cols, Some(5));
        assert_eq!(cmd.cell_rows, Some(3));
    }

    // -- Kitty accumulator --

    #[test]
    fn test_kitty_accumulator_single_chunk() {
        let mut acc = KittyAccumulator::new();
        // 2x1 RGBA image: red + blue pixels, base64 encoded.
        let pixel_data: [u8; 8] = [255, 0, 0, 255, 0, 0, 255, 255];
        let b64 = base64::engine::general_purpose::STANDARD.encode(pixel_data);

        let result = acc.feed("a=T,f=32,s=2,v=1,i=1", &b64);
        assert!(result.is_some());

        let (cmd, data) = result.unwrap();
        assert_eq!(cmd.action, KittyAction::TransmitAndDisplay);
        assert_eq!(data.len(), 8);
        assert_eq!(data, pixel_data.to_vec());
    }

    #[test]
    fn test_kitty_accumulator_chunked() {
        let mut acc = KittyAccumulator::new();
        let pixel_data: [u8; 8] = [255, 0, 0, 255, 0, 255, 0, 255];
        let b64 = base64::engine::general_purpose::STANDARD.encode(pixel_data);

        // Split base64 string into two chunks.
        let mid = b64.len() / 2;
        let chunk1 = &b64[..mid];
        let chunk2 = &b64[mid..];

        let result1 = acc.feed("a=T,f=32,s=2,v=1,i=2,m=1", chunk1);
        assert!(result1.is_none());

        let result2 = acc.feed("m=0", chunk2);
        assert!(result2.is_some());

        let (cmd, data) = result2.unwrap();
        assert_eq!(cmd.action, KittyAction::TransmitAndDisplay);
        assert_eq!(data, pixel_data.to_vec());
    }

    // -- Image store --

    #[test]
    fn test_image_store_basic() {
        let mut store = ImageStore::new();
        store.set_cell_size(8, 16);

        let img = TerminalImage {
            id: 1,
            data: vec![255; 32 * 16 * 4],
            width: 32,
            height: 16,
        };
        store.store_image(img);

        assert!(store.get_image(1).is_some());
        assert!(store.get_image(2).is_none());

        store.add_placement(ImagePlacement {
            image_id: 1,
            col: 0,
            row: 0,
            cell_cols: 0,
            cell_rows: 0,
            src_width: 32,
            src_height: 16,
        });

        assert_eq!(store.placements().len(), 1);
        assert_eq!(store.placements()[0].cell_cols, 4); // 32 / 8
        assert_eq!(store.placements()[0].cell_rows, 1); // 16 / 16

        store.delete_image(1);
        assert!(store.get_image(1).is_none());
        assert_eq!(store.placements().len(), 0);
    }

    #[test]
    fn test_image_store_auto_id() {
        let mut store = ImageStore::new();
        assert_eq!(store.next_id(), 1);
        assert_eq!(store.next_id(), 2);
        assert_eq!(store.next_id(), 3);
    }

    #[test]
    fn test_image_store_delete_all() {
        let mut store = ImageStore::new();
        store.store_image(TerminalImage {
            id: 1,
            data: vec![0; 16],
            width: 2,
            height: 2,
        });
        store.store_image(TerminalImage {
            id: 2,
            data: vec![0; 16],
            width: 2,
            height: 2,
        });
        store.delete_all();
        assert!(store.images().is_empty());
        assert!(store.placements().is_empty());
    }

    // -- Sixel decoding --

    #[test]
    fn test_sixel_decode_simple() {
        // A simple 1x6 red column: sixel char `~` = 0x7E - 0x3F = 0x3F = 63 = all 6 bits set.
        // Set color 0 to red, then draw one `~` character.
        let data = b"#0;2;100;0;0~";
        let result = decode_sixel(data);
        assert!(result.is_some());
        let img = result.unwrap();
        assert_eq!(img.width, 1);
        assert_eq!(img.height, 6);
        // Check first pixel is red.
        assert_eq!(img.data[0], 255); // R
        assert_eq!(img.data[1], 0);   // G
        assert_eq!(img.data[2], 0);   // B
        assert_eq!(img.data[3], 255); // A
    }

    #[test]
    fn test_sixel_decode_repeat() {
        // Draw 3 columns of all-on pixels using repeat syntax.
        let data = b"#0;2;0;100;0!3~";
        let result = decode_sixel(data);
        assert!(result.is_some());
        let img = result.unwrap();
        assert_eq!(img.width, 3);
        assert_eq!(img.height, 6);
        // Check pixel at (2, 0) is green.
        let offset = (0 * 3 + 2) * 4;
        assert_eq!(img.data[offset as usize], 0);     // R
        assert_eq!(img.data[offset as usize + 1], 255); // G
        assert_eq!(img.data[offset as usize + 2], 0);   // B
    }

    #[test]
    fn test_sixel_decode_newline() {
        // Two rows of sixels separated by `-`.
        let data = b"#0;2;100;100;100~-~";
        let result = decode_sixel(data);
        assert!(result.is_some());
        let img = result.unwrap();
        assert_eq!(img.width, 1);
        assert_eq!(img.height, 12);
    }

    #[test]
    fn test_sixel_decode_partial_bits() {
        // Sixel `@` = 0x40 - 0x3F = 1 = only bit 0 set (top pixel only).
        let data = b"#0;2;100;100;0@";
        let result = decode_sixel(data);
        assert!(result.is_some());
        let img = result.unwrap();
        assert_eq!(img.width, 1);
        assert_eq!(img.height, 6);
        // Pixel (0, 0) should be yellow.
        assert_eq!(img.data[0], 255);
        assert_eq!(img.data[1], 255);
        assert_eq!(img.data[2], 0);
        assert_eq!(img.data[3], 255);
        // Pixel (0, 1) should be transparent (unset).
        assert_eq!(img.data[7], 0); // A channel of pixel (0,1)
    }

    #[test]
    fn test_sixel_decode_empty() {
        let result = decode_sixel(b"");
        assert!(result.is_none());
    }

    // -- APC extractor --

    #[test]
    fn test_apc_extractor_basic() {
        let mut ext = ApcExtractor::new();
        // ESC _ G payload ESC backslash
        let data = b"\x1b_Ghello\x1b\\world";
        let result = ext.process(data);
        assert_eq!(result.apc_payloads.len(), 1);
        assert_eq!(result.apc_payloads[0], b"Ghello");
        assert_eq!(result.passthrough, b"world");
    }

    #[test]
    fn test_apc_extractor_no_apc() {
        let mut ext = ApcExtractor::new();
        let data = b"Hello, world!";
        let result = ext.process(data);
        assert!(result.apc_payloads.is_empty());
        assert_eq!(result.passthrough, data.to_vec());
    }

    #[test]
    fn test_apc_extractor_esc_not_apc() {
        let mut ext = ApcExtractor::new();
        // ESC [ is CSI, not APC.
        let data = b"\x1b[1;2H";
        let result = ext.process(data);
        assert!(result.apc_payloads.is_empty());
        assert_eq!(result.passthrough, data.to_vec());
    }

    #[test]
    fn test_apc_extractor_multiple_apc() {
        let mut ext = ApcExtractor::new();
        let data = b"\x1b_Gfirst\x1b\\between\x1b_Gsecond\x1b\\after";
        let result = ext.process(data);
        assert_eq!(result.apc_payloads.len(), 2);
        assert_eq!(result.apc_payloads[0], b"Gfirst");
        assert_eq!(result.apc_payloads[1], b"Gsecond");
        assert_eq!(result.passthrough, b"betweenafter");
    }

    #[test]
    fn test_apc_extractor_split_across_chunks() {
        let mut ext = ApcExtractor::new();
        // First chunk: start of APC.
        let result1 = ext.process(b"\x1b_Gpar");
        assert!(result1.apc_payloads.is_empty());
        // Second chunk: end of APC.
        let result2 = ext.process(b"tial\x1b\\done");
        assert_eq!(result2.apc_payloads.len(), 1);
        assert_eq!(result2.apc_payloads[0], b"Gpartial");
        assert_eq!(result2.passthrough, b"done");
    }

    #[test]
    fn test_apc_extractor_st_c1() {
        let mut ext = ApcExtractor::new();
        // Using C1 ST (0x9C) instead of ESC \.
        let data = b"\x1b_Gdata\x9crest";
        let result = ext.process(data);
        assert_eq!(result.apc_payloads.len(), 1);
        assert_eq!(result.apc_payloads[0], b"Gdata");
        assert_eq!(result.passthrough, b"rest");
    }

    // -- Kitty full pipeline --

    #[test]
    fn test_kitty_process_rgba() {
        let mut store = ImageStore::new();
        store.set_cell_size(8, 16);

        let pixels: Vec<u8> = vec![
            255, 0, 0, 255, // red
            0, 255, 0, 255, // green
            0, 0, 255, 255, // blue
            255, 255, 0, 255, // yellow
        ];

        let cmd = KittyCommand {
            action: KittyAction::TransmitAndDisplay,
            format: KittyFormat::Rgba,
            image_id: Some(10),
            width: Some(2),
            height: Some(2),
            transmission: 'd',
            ..KittyCommand::default()
        };

        process_kitty_command(&cmd, &pixels, &mut store, 5, 3);

        assert!(store.get_image(10).is_some());
        let img = store.get_image(10).unwrap();
        assert_eq!(img.width, 2);
        assert_eq!(img.height, 2);
        assert_eq!(img.data.len(), 16);

        assert_eq!(store.placements().len(), 1);
        assert_eq!(store.placements()[0].col, 5);
        assert_eq!(store.placements()[0].row, 3);
    }

    #[test]
    fn test_kitty_process_rgb() {
        let mut store = ImageStore::new();
        store.set_cell_size(8, 16);

        let pixels: Vec<u8> = vec![
            255, 0, 0, // red
            0, 255, 0, // green
        ];

        let cmd = KittyCommand {
            action: KittyAction::TransmitAndDisplay,
            format: KittyFormat::Rgb,
            image_id: Some(20),
            width: Some(2),
            height: Some(1),
            transmission: 'd',
            ..KittyCommand::default()
        };

        process_kitty_command(&cmd, &pixels, &mut store, 0, 0);

        let img = store.get_image(20).unwrap();
        assert_eq!(img.data.len(), 8); // 2 pixels * 4 bytes (RGBA)
        assert_eq!(img.data[3], 255);  // Alpha added
        assert_eq!(img.data[7], 255);
    }

    #[test]
    fn test_kitty_process_delete() {
        let mut store = ImageStore::new();
        store.store_image(TerminalImage {
            id: 1,
            data: vec![0; 16],
            width: 2,
            height: 2,
        });
        store.add_placement(ImagePlacement {
            image_id: 1,
            col: 0,
            row: 0,
            cell_cols: 1,
            cell_rows: 1,
            src_width: 2,
            src_height: 2,
        });

        let cmd = KittyCommand {
            action: KittyAction::Delete,
            image_id: Some(1),
            delete_target: Some('i'),
            ..KittyCommand::default()
        };

        process_kitty_command(&cmd, &[], &mut store, 0, 0);
        assert!(store.get_image(1).is_none());
        assert!(store.placements().is_empty());
    }

    #[test]
    fn test_kitty_transmit_then_place() {
        let mut store = ImageStore::new();
        store.set_cell_size(8, 16);

        let pixels: Vec<u8> = vec![0; 16]; // 2x2 RGBA

        // Transmit only.
        let cmd_t = KittyCommand {
            action: KittyAction::Transmit,
            format: KittyFormat::Rgba,
            image_id: Some(5),
            width: Some(2),
            height: Some(2),
            transmission: 'd',
            ..KittyCommand::default()
        };
        process_kitty_command(&cmd_t, &pixels, &mut store, 0, 0);
        assert!(store.get_image(5).is_some());
        assert!(store.placements().is_empty());

        // Place.
        let cmd_p = KittyCommand {
            action: KittyAction::Place,
            image_id: Some(5),
            cell_cols: Some(3),
            cell_rows: Some(2),
            ..KittyCommand::default()
        };
        process_kitty_command(&cmd_p, &[], &mut store, 10, 5);
        assert_eq!(store.placements().len(), 1);
        assert_eq!(store.placements()[0].col, 10);
        assert_eq!(store.placements()[0].row, 5);
        assert_eq!(store.placements()[0].cell_cols, 3);
        assert_eq!(store.placements()[0].cell_rows, 2);
    }
}
