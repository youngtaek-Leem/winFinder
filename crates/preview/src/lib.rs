use std::fs::File;
use std::io::Read;
use std::path::Path;

pub const MAX_TEXT_BYTES: u64 = 4 * 1024 * 1024;
pub const MAX_IMAGE_DIMENSION: u32 = 1600;
pub const MAX_PDF_WIDTH: f32 = 1400.0;

#[derive(Clone, Debug)]
pub enum Preview {
    Text {
        text: String,
        truncated: bool,
        encoding: String,
    },
    Markdown(String),
    Image {
        rgba: Vec<u8>,
        width: u32,
        height: u32,
    },
    Pdf {
        rgba: Vec<u8>,
        width: u32,
        height: u32,
        pages: usize,
    },
    Audio {
        info: String,
    },
    Video {
        info: String,
    },
    Info {
        kind: String,
        info: String,
    },
    Error(String),
}

pub fn load(path: &Path) -> Preview {
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_lowercase();

    match extension.as_str() {
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "tif" | "tiff" | "ico" | "avif" => {
            load_image(path)
        }
        "md" | "markdown" | "mdx" => match load_text(path) {
            Ok((text, truncated, encoding)) => {
                if truncated {
                    Preview::Text {
                        text,
                        truncated,
                        encoding,
                    }
                } else {
                    Preview::Markdown(text)
                }
            }
            Err(err) => Preview::Error(err),
        },
        "txt" | "log" | "json" | "toml" | "yaml" | "yml" | "xml" | "csv" | "tsv" | "rs" | "py"
        | "js" | "ts" | "tsx" | "jsx" | "c" | "h" | "cpp" | "hpp" | "cs" | "java" | "go" | "rb"
        | "php" | "sh" | "bat" | "ps1" | "ini" | "cfg" | "conf" | "sql" | "css" | "html"
        | "htm" => match load_text(path) {
            Ok((text, truncated, encoding)) => Preview::Text {
                text,
                truncated,
                encoding,
            },
            Err(err) => Preview::Error(err),
        },
        "pdf" => load_pdf(path),
        "mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac" | "opus" | "wma" => Preview::Audio {
            info: file_info(path),
        },
        "mp4" | "mkv" | "mov" | "avi" | "webm" | "wmv" | "m4v" | "mpg" | "mpeg" => Preview::Video {
            info: format!(
                "{}\n\nWindows Media Foundation 재생 지원 예정",
                file_info(path)
            ),
        },
        _ => Preview::Info {
            kind: if extension.is_empty() {
                "File".to_string()
            } else {
                extension.to_uppercase()
            },
            info: file_info(path),
        },
    }
}

fn load_text(path: &Path) -> Result<(String, bool, String), String> {
    let metadata = std::fs::metadata(path).map_err(|err| err.to_string())?;
    let truncated = metadata.len() > MAX_TEXT_BYTES;
    let mut file = File::open(path).map_err(|err| err.to_string())?;
    let mut bytes = Vec::with_capacity(metadata.len().min(MAX_TEXT_BYTES) as usize);
    file.by_ref()
        .take(MAX_TEXT_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|err| err.to_string())?;

    if let Ok(text) = String::from_utf8(bytes.clone()) {
        return Ok((text, truncated, "UTF-8".to_string()));
    }

    let mut detector = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Deny);
    detector.feed(&bytes, true);
    let encoding = detector.guess(None, chardetng::Utf8Detection::Allow);
    let (text, _, _) = encoding.decode(&bytes);
    Ok((text.into_owned(), truncated, encoding.name().to_string()))
}

fn load_image(path: &Path) -> Preview {
    let reader = match image::ImageReader::open(path) {
        Ok(reader) => reader,
        Err(err) => return Preview::Error(err.to_string()),
    };
    let reader = match reader.with_guessed_format() {
        Ok(reader) => reader,
        Err(err) => return Preview::Error(err.to_string()),
    };
    let image = match reader.decode() {
        Ok(image) => image,
        Err(err) => return Preview::Error(err.to_string()),
    };
    let image = image.thumbnail(MAX_IMAGE_DIMENSION, MAX_IMAGE_DIMENSION);
    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    Preview::Image {
        rgba: rgba.into_raw(),
        width,
        height,
    }
}

fn load_pdf(path: &Path) -> Preview {
    let result = (|| -> Result<Preview, String> {
        let pdfium = pdfium_render::prelude::Pdfium::bind_to_system_library()
            .map_err(|err| format!("pdfium 로드 실패: {err}"))?;
        let pdfium = pdfium_render::prelude::Pdfium::new(pdfium);
        let document = pdfium
            .load_pdf_from_file(path, None)
            .map_err(|err| format!("PDF 열기 실패: {err}"))?;
        let pages = document.pages().len() as usize;
        if pages == 0 {
            return Err("빈 PDF 문서입니다".to_string());
        }
        let page = document.pages().get(0).map_err(|err| err.to_string())?;
        let config =
            pdfium_render::prelude::PdfRenderConfig::new().set_target_width(MAX_PDF_WIDTH as i32);
        let bitmap = page
            .render_with_config(&config)
            .map_err(|err| err.to_string())?
            .as_image()
            .map_err(|err| err.to_string())?;
        let rgba = bitmap.to_rgba8();
        let (width, height) = rgba.dimensions();
        Ok(Preview::Pdf {
            rgba: rgba.into_raw(),
            width,
            height,
            pages,
        })
    })();
    result.unwrap_or_else(Preview::Error)
}

fn file_info(path: &Path) -> String {
    let metadata = std::fs::metadata(path);
    let size = metadata
        .as_ref()
        .map(|m| format_size(m.len()))
        .unwrap_or_else(|_| "-".to_string());
    format!("{}\n{}", path.display(), size)
}

pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_sizes() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1024 * 1024), "1.0 MB");
    }
}
