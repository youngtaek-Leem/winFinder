#[cfg(windows)]
mod imp {
    use rodio::buffer::SamplesBuffer;
    use rodio::{OutputStream, Sink};
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};
    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
    use windows::core::PCWSTR;

    const MAX_AUDIO_PCM_BYTES: usize = 128 * 1024 * 1024;

    pub struct VideoFrame {
        pub width: u32,
        pub height: u32,
        pub rgba: Vec<u8>,
    }

    struct Shared {
        frame: Mutex<Option<VideoFrame>>,
        error: Mutex<Option<String>>,
        finished: AtomicBool,
    }

    pub struct VideoPlayer {
        shared: Arc<Shared>,
        stop: Arc<AtomicBool>,
        paused: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl VideoPlayer {
        pub fn open(path: &Path) -> Result<Self, String> {
            let shared = Arc::new(Shared {
                frame: Mutex::new(None),
                error: Mutex::new(None),
                finished: AtomicBool::new(false),
            });
            let stop = Arc::new(AtomicBool::new(false));
            let paused = Arc::new(AtomicBool::new(false));

            let thread_shared = Arc::clone(&shared);
            let thread_stop = Arc::clone(&stop);
            let thread_paused = Arc::clone(&paused);
            let path = path.to_path_buf();
            let thread = thread::Builder::new()
                .name("winfinder-video".into())
                .spawn(move || {
                    let result = playback(&path, &thread_shared, &thread_stop, &thread_paused);
                    if let Err(error) = result
                        && !thread_stop.load(Ordering::Relaxed)
                        && let Ok(mut slot) = thread_shared.error.lock()
                    {
                        *slot = Some(error);
                    }
                    thread_shared.finished.store(true, Ordering::Relaxed);
                })
                .map_err(|error| error.to_string())?;

            Ok(Self {
                shared,
                stop,
                paused,
                thread: Some(thread),
            })
        }

        pub fn take_frame(&self) -> Option<VideoFrame> {
            self.shared.frame.lock().ok()?.take()
        }

        pub fn error(&self) -> Option<String> {
            self.shared.error.lock().ok()?.clone()
        }

        pub fn is_paused(&self) -> bool {
            self.paused.load(Ordering::Relaxed)
        }

        pub fn is_finished(&self) -> bool {
            self.shared.finished.load(Ordering::Relaxed)
        }

        pub fn toggle(&self) {
            if !self.is_finished() {
                self.paused.fetch_xor(true, Ordering::Relaxed);
            }
        }
    }

    impl Drop for VideoPlayer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn media_foundation_available() -> bool {
        use windows::Win32::System::LibraryLoader::LoadLibraryW;
        use windows::core::w;

        unsafe {
            LoadLibraryW(w!("mfplat.dll")).is_ok() && LoadLibraryW(w!("mfreadwrite.dll")).is_ok()
        }
    }

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    fn with_media_foundation<T>(body: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        unsafe {
            let co_initialized = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
            if let Err(error) = MFStartup(MF_VERSION, MFSTARTUP_LITE) {
                if co_initialized {
                    CoUninitialize();
                }
                return Err(format!("Media Foundation 시작 실패: {error}"));
            }
            let result = body();
            let _ = MFShutdown();
            if co_initialized {
                CoUninitialize();
            }
            result
        }
    }

    fn playback(
        path: &Path,
        shared: &Shared,
        stop: &AtomicBool,
        paused: &AtomicBool,
    ) -> Result<(), String> {
        if !media_foundation_available() {
            return Err("동영상 재생에 필요한 Windows Media Foundation을 찾을 수 없습니다".into());
        }
        with_media_foundation(|| {
            let audio = if stop.load(Ordering::Relaxed) {
                None
            } else {
                decode_audio(path, stop).ok().flatten()
            };
            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }

            let mut stream = None;
            let mut sink = None;
            if let Some((sample_rate, channels, pcm)) = audio
                && let Ok((output, handle)) = OutputStream::try_default()
                && let Ok(player) = Sink::try_new(&handle)
            {
                player.append(SamplesBuffer::new(channels, sample_rate, pcm));
                player.pause();
                sink = Some(player);
                stream = Some(output);
            }

            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }
            let result = decode_video(path, shared, stop, paused, sink.as_ref());
            if let Some(sink) = &sink {
                sink.stop();
            }
            drop(stream);
            result
        })
    }

    fn decode_video(
        path: &Path,
        shared: &Shared,
        stop: &AtomicBool,
        paused: &AtomicBool,
        sink: Option<&Sink>,
    ) -> Result<(), String> {
        unsafe {
            let reader = open_reader(path)?;
            let (width, height, stride) = negotiate_rgb32(&reader)?;
            let video = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
            let row_bytes = width as usize * 4;
            let mut rgba = vec![0u8; row_bytes * height as usize];
            let mut clock = Instant::now();
            let mut base_ts = 0i64;
            let mut first_frame = true;
            let mut paused_total = Duration::ZERO;
            let mut pause_started: Option<Instant> = None;
            let mut audio_started = false;

            loop {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                if paused.load(Ordering::Relaxed) {
                    if pause_started.is_none() {
                        pause_started = Some(Instant::now());
                        if let Some(sink) = sink {
                            sink.pause();
                        }
                    }
                    thread::sleep(Duration::from_millis(20));
                    continue;
                }
                if let Some(started) = pause_started.take() {
                    paused_total += started.elapsed();
                    if let Some(sink) = sink
                        && audio_started
                    {
                        sink.play();
                    }
                }

                let mut flags = 0u32;
                let mut timestamp = 0i64;
                let mut sample: Option<IMFSample> = None;
                reader
                    .ReadSample(
                        video,
                        0,
                        None,
                        Some(&mut flags),
                        Some(&mut timestamp),
                        Some(&mut sample),
                    )
                    .map_err(|error| format!("영상 프레임 읽기 실패: {error}"))?;

                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    break;
                }
                let Some(sample) = sample else { continue };

                let buffer = sample
                    .ConvertToContiguousBuffer()
                    .map_err(|error| format!("영상 버퍼 변환 실패: {error}"))?;
                let mut ptr: *mut u8 = std::ptr::null_mut();
                let mut len = 0u32;
                buffer
                    .Lock(&mut ptr, None, Some(&mut len))
                    .map_err(|error| format!("영상 버퍼 잠금 실패: {error}"))?;
                let copied = copy_frame(
                    std::slice::from_raw_parts(ptr, len as usize),
                    &mut rgba,
                    width,
                    height,
                    stride,
                );
                let _ = buffer.Unlock();
                copied?;

                if first_frame {
                    first_frame = false;
                    base_ts = timestamp;
                    clock = Instant::now();
                    if let Some(sink) = sink {
                        sink.play();
                        audio_started = true;
                    }
                }
                let target =
                    Duration::from_micros(timestamp.saturating_sub(base_ts).max(0) as u64 / 10)
                        + paused_total;
                wait_until(clock + target, stop, paused);
                if stop.load(Ordering::Relaxed) {
                    break;
                }

                if let Ok(mut slot) = shared.frame.lock() {
                    *slot = Some(VideoFrame {
                        width,
                        height,
                        rgba: rgba.clone(),
                    });
                }
            }

            Ok(())
        }
    }

    fn wait_until(deadline: Instant, stop: &AtomicBool, paused: &AtomicBool) {
        while !stop.load(Ordering::Relaxed) && !paused.load(Ordering::Relaxed) {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            thread::sleep((deadline - now).min(Duration::from_millis(10)));
        }
    }

    unsafe fn open_reader(path: &Path) -> Result<IMFSourceReader, String> {
        let mut attributes: Option<IMFAttributes> = None;
        unsafe {
            MFCreateAttributes(&mut attributes, 2)
                .map_err(|error| format!("Media Foundation 속성 생성 실패: {error}"))?;
        }
        let attributes = attributes.ok_or("Media Foundation 속성 생성 실패")?;
        unsafe {
            attributes
                .SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)
                .map_err(|error| format!("영상 처리 활성화 실패: {error}"))?;
        }
        let url = wide(path);
        unsafe {
            MFCreateSourceReaderFromURL(PCWSTR(url.as_ptr()), &attributes)
                .map_err(|error| format!("영상 열기 실패: {error}"))
        }
    }

    unsafe fn negotiate_rgb32(reader: &IMFSourceReader) -> Result<(u32, u32, i32), String> {
        unsafe {
            let video = MF_SOURCE_READER_FIRST_VIDEO_STREAM.0 as u32;
            reader
                .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
                .map_err(|error| format!("영상 스트림 선택 실패: {error}"))?;
            reader
                .SetStreamSelection(video, true)
                .map_err(|_| "영상 스트림이 없습니다".to_string())?;

            let wanted =
                MFCreateMediaType().map_err(|error| format!("미디어 타입 생성 실패: {error}"))?;
            wanted
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|error| error.to_string())?;
            wanted
                .SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)
                .map_err(|error| error.to_string())?;
            reader
                .SetCurrentMediaType(video, None, &wanted)
                .map_err(|error| format!("RGB32 디코딩을 사용할 수 없습니다: {error}"))?;

            let current = reader
                .GetCurrentMediaType(video)
                .map_err(|error| format!("영상 타입 확인 실패: {error}"))?;
            let size = current
                .GetUINT64(&MF_MT_FRAME_SIZE)
                .map_err(|error| format!("영상 크기 확인 실패: {error}"))?;
            let width = (size >> 32) as u32;
            let height = (size & 0xFFFF_FFFF) as u32;
            if width == 0 || height == 0 {
                return Err("영상 크기가 0입니다".into());
            }
            let stride = current
                .GetUINT32(&MF_MT_DEFAULT_STRIDE)
                .map(|value| value as i32)
                .unwrap_or((width * 4) as i32);
            Ok((width, height, stride))
        }
    }

    fn copy_frame(
        src: &[u8],
        dst: &mut [u8],
        width: u32,
        height: u32,
        stride: i32,
    ) -> Result<(), String> {
        let row_bytes = width as usize * 4;
        let pitch = stride.unsigned_abs() as usize;
        if pitch < row_bytes {
            return Err(format!("영상 stride 오류: {pitch} < {row_bytes}"));
        }
        let needed = pitch * (height as usize - 1) + row_bytes;
        if src.len() < needed {
            return Err(format!("영상 버퍼 부족: {} < {needed}", src.len()));
        }
        for y in 0..height as usize {
            let src_row = if stride < 0 {
                height as usize - 1 - y
            } else {
                y
            };
            let src_offset = src_row * pitch;
            let dst_offset = y * row_bytes;
            for x in 0..width as usize {
                let s = src_offset + x * 4;
                let d = dst_offset + x * 4;
                dst[d] = src[s + 2];
                dst[d + 1] = src[s + 1];
                dst[d + 2] = src[s];
                dst[d + 3] = 255;
            }
        }
        Ok(())
    }

    fn decode_audio(
        path: &Path,
        stop: &AtomicBool,
    ) -> Result<Option<(u32, u16, Vec<i16>)>, String> {
        unsafe {
            let reader = open_reader(path)?;
            let audio = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
            reader
                .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
                .map_err(|error| format!("오디오 스트림 선택 실패: {error}"))?;
            if reader.SetStreamSelection(audio, true).is_err() {
                return Ok(None);
            }

            let wanted =
                MFCreateMediaType().map_err(|error| format!("미디어 타입 생성 실패: {error}"))?;
            wanted
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
                .map_err(|error| error.to_string())?;
            wanted
                .SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)
                .map_err(|error| error.to_string())?;
            wanted
                .SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)
                .map_err(|error| error.to_string())?;
            if reader.SetCurrentMediaType(audio, None, &wanted).is_err() {
                return Ok(None);
            }

            let current = reader
                .GetCurrentMediaType(audio)
                .map_err(|error| format!("오디오 타입 확인 실패: {error}"))?;
            let sample_rate = current
                .GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)
                .map_err(|error| format!("오디오 샘플레이트 확인 실패: {error}"))?;
            let channels = current
                .GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS)
                .map_err(|error| format!("오디오 채널 확인 실패: {error}"))?
                as u16;
            if channels == 0 {
                return Ok(None);
            }

            let mut pcm = Vec::new();
            loop {
                if stop.load(Ordering::Relaxed) {
                    return Ok(None);
                }
                let mut flags = 0u32;
                let mut sample: Option<IMFSample> = None;
                reader
                    .ReadSample(audio, 0, None, Some(&mut flags), None, Some(&mut sample))
                    .map_err(|error| format!("오디오 읽기 실패: {error}"))?;
                if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                    break;
                }
                let Some(sample) = sample else { continue };
                let buffer = sample
                    .ConvertToContiguousBuffer()
                    .map_err(|error| format!("오디오 버퍼 변환 실패: {error}"))?;
                let mut ptr: *mut u8 = std::ptr::null_mut();
                let mut len = 0u32;
                buffer
                    .Lock(&mut ptr, None, Some(&mut len))
                    .map_err(|error| format!("오디오 버퍼 잠금 실패: {error}"))?;
                let bytes = std::slice::from_raw_parts(ptr, len as usize);
                let too_large = pcm.len() + bytes.len() / 2 > MAX_AUDIO_PCM_BYTES / 2;
                if !too_large {
                    let (pairs, _) = bytes.as_chunks::<2>();
                    pcm.extend(pairs.iter().map(|pair| i16::from_le_bytes(*pair)));
                }
                let _ = buffer.Unlock();
                if too_large {
                    return Ok(None);
                }
            }

            if pcm.is_empty() {
                Ok(None)
            } else {
                Ok(Some((sample_rate, channels, pcm)))
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::path::PathBuf;

        fn fixture(name: &str) -> PathBuf {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../tests/fixtures")
                .join(name)
        }

        #[test]
        fn decodes_video_frames() {
            let path = fixture("sample.wmv");
            let shared = Shared {
                frame: Mutex::new(None),
                error: Mutex::new(None),
                finished: AtomicBool::new(false),
            };
            let stop = AtomicBool::new(false);
            let paused = AtomicBool::new(false);
            with_media_foundation(|| decode_video(&path, &shared, &stop, &paused, None))
                .expect("decode");
            let frame = shared.frame.lock().unwrap().take().expect("frame");
            assert_eq!(frame.width, 160);
            assert_eq!(frame.height, 120);
            assert_eq!(frame.rgba.len(), 160 * 120 * 4);
        }

        #[test]
        fn decodes_audio_samples() {
            let path = fixture("sample.wav");
            let stop = AtomicBool::new(false);
            let (sample_rate, channels, pcm) = with_media_foundation(|| decode_audio(&path, &stop))
                .expect("decode")
                .expect("audio");
            assert!(sample_rate > 0);
            assert!(channels > 0);
            assert!(!pcm.is_empty());
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::path::Path;

    pub struct VideoFrame {
        pub width: u32,
        pub height: u32,
        pub rgba: Vec<u8>,
    }

    pub struct VideoPlayer;

    impl VideoPlayer {
        pub fn open(_path: &Path) -> Result<Self, String> {
            Err("동영상 재생은 Windows 빌드에서 지원됩니다".into())
        }

        pub fn take_frame(&self) -> Option<VideoFrame> {
            None
        }

        pub fn error(&self) -> Option<String> {
            None
        }

        pub fn is_paused(&self) -> bool {
            false
        }

        pub fn is_finished(&self) -> bool {
            true
        }

        pub fn toggle(&self) {}
    }
}

pub use imp::VideoPlayer;
