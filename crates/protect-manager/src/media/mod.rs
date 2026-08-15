//! Getting clips to a browser.
//!
//! The recordings are HEVC, which most browsers will not play: Firefox has no
//! HEVC support on Linux at all, and Chromium's depends on platform hardware
//! decoding. So a clip is probed once, and if the browser cannot be expected
//! to play it, it is transcoded to H.264 and the result is cached.
//!
//! Everything here is on-demand rather than pre-generated. The live window is
//! thousands of clips and almost none of them are ever watched; transcoding
//! them all would burn hours of CPU on a NAS to produce files nobody opens.

pub mod range;

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use tokio::process::Command;
use tokio::sync::Semaphore;

/// Codecs a browser can be relied on to play from an MP4 container.
const BROWSER_SAFE: &[&str] = &["h264", "avc1", "vp8", "vp9", "av1"];

#[derive(Clone)]
pub struct Media {
    pub cache_dir: PathBuf,
    /// ffmpeg is CPU-hungry and this often runs on a NAS that is also serving
    /// files. Two at a time keeps a browsing session responsive without
    /// letting a page of thumbnails saturate the machine.
    limiter: Arc<Semaphore>,
}

impl Media {
    pub fn new(cache_dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(cache_dir.join("thumbs"));
        let _ = std::fs::create_dir_all(cache_dir.join("clips"));
        Self { cache_dir, limiter: Arc::new(Semaphore::new(2)) }
    }

    fn thumb_path(&self, id: &str) -> PathBuf {
        self.cache_dir.join("thumbs").join(format!("{}.jpg", safe_name(id)))
    }

    fn clip_path(&self, id: &str) -> PathBuf {
        self.cache_dir.join("clips").join(format!("{}.mp4", safe_name(id)))
    }

    /// A scratch name for a file being written.
    ///
    /// The extension stays last: ffmpeg picks its output format from it, and
    /// a name ending in `.partial` leaves it unable to choose a muxer at all.
    fn partial(&self, dest: &Path, ext: &str) -> PathBuf {
        let stem = dest.file_stem().unwrap_or_default().to_string_lossy().to_string();
        dest.with_file_name(format!("{stem}.partial.{ext}"))
    }

    /// What a clip's video stream is: codec and dimensions.
    ///
    /// One ffprobe call for all three, because spawning the process dominates
    /// the cost and asking twice would double it for no reason.
    pub async fn probe(&self, source: &Path) -> anyhow::Result<VideoInfo> {
        let out = Command::new("ffprobe")
            .args([
                "-v", "error",
                "-select_streams", "v:0",
                "-show_entries", "stream=codec_name,width,height,r_frame_rate",
                "-of", "csv=p=0:nk=1",
            ])
            .arg(source)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await?;

        if !out.status.success() {
            anyhow::bail!("ffprobe failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        }

        let line = String::from_utf8_lossy(&out.stdout);
        let mut parts = line.trim().split(',');
        Ok(VideoInfo {
            codec: parts.next().unwrap_or_default().trim().to_lowercase(),
            width: parts.next().and_then(|v| v.trim().parse().ok()),
            height: parts.next().and_then(|v| v.trim().parse().ok()),
            fps: parts.next().and_then(parse_frame_rate),
        })
    }

    pub async fn probe_codec(&self, source: &Path) -> anyhow::Result<String> {
        Ok(self.probe(source).await?.codec)
    }

    pub fn browser_can_play(codec: &str) -> bool {
        BROWSER_SAFE.contains(&codec)
    }

    /// A cached still from the clip, generating it if needed.
    pub async fn thumbnail(&self, id: &str, source: &Path) -> anyhow::Result<PathBuf> {
        let dest = self.thumb_path(id);
        if dest.is_file() {
            return Ok(dest);
        }

        let _permit = self.limiter.acquire().await?;
        // Re-check after waiting: another request may have produced it while
        // this one sat in the queue.
        if dest.is_file() {
            return Ok(dest);
        }

        let tmp = self.partial(&dest, "jpg");
        let status = Command::new("ffmpeg")
            .args(["-y", "-loglevel", "error"])
            // A second in, because the first frame of a motion clip is often
            // the empty scene just before whatever triggered it.
            .args(["-ss", "1"])
            .arg("-i")
            .arg(source)
            .args(["-frames:v", "1", "-vf", "scale=320:-2", "-q:v", "5"])
            .arg(&tmp)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .await?;

        if !status.status.success() || !tmp.is_file() {
            let _ = std::fs::remove_file(&tmp);
            anyhow::bail!(
                "could not make a thumbnail: {}",
                String::from_utf8_lossy(&status.stderr).trim()
            );
        }
        std::fs::rename(&tmp, &dest)?;
        Ok(dest)
    }

    /// A file the browser can play — the original when possible, a cached
    /// transcode when not.
    pub async fn playable(&self, id: &str, source: &Path) -> anyhow::Result<PlayableClip> {
        let codec = self.probe_codec(source).await.unwrap_or_default();
        if Self::browser_can_play(&codec) {
            return Ok(PlayableClip { path: source.to_path_buf(), transcoded: false, codec });
        }

        let dest = self.clip_path(id);
        if dest.is_file() {
            return Ok(PlayableClip { path: dest, transcoded: true, codec });
        }

        let _permit = self.limiter.acquire().await?;
        if dest.is_file() {
            return Ok(PlayableClip { path: dest, transcoded: true, codec });
        }

        let tmp = self.partial(&dest, "mp4");
        let out = Command::new("ffmpeg")
            .args(["-y", "-loglevel", "error"])
            .arg("-i")
            .arg(source)
            // 1080p is more than a review player needs from a 4 MP camera, and
            // halves the encode. `-c:a copy` because the audio is already AAC,
            // and re-encoding it would be pure waste.
            .args([
                "-vf", "scale=-2:min(1080\\,ih)",
                "-c:v", "libx264",
                "-preset", "veryfast",
                "-crf", "23",
                "-c:a", "copy",
                // Without this the index sits at the end of the file and the
                // browser must download all of it before playing anything.
                "-movflags", "+faststart",
            ])
            .arg(&tmp)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .await?;

        if !out.status.success() || !tmp.is_file() {
            let _ = std::fs::remove_file(&tmp);
            anyhow::bail!(
                "could not transcode: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        std::fs::rename(&tmp, &dest)?;
        Ok(PlayableClip { path: dest, transcoded: true, codec })
    }

    /// Whether a transcode already exists, so the UI can say "preparing"
    /// rather than leaving a video element spinning with no explanation.
    pub fn is_prepared(&self, id: &str) -> bool {
        self.clip_path(id).is_file()
    }

    /// Drop cached derivatives for clips that are no longer live.
    ///
    /// The cache only ever covers the live window, so archiving a month makes
    /// its thumbnails and transcodes dead weight. Called after a sync rather
    /// than on a timer — the index is what knows which clips still exist.
    pub fn evict(&self, keep: &std::collections::HashSet<String>) -> usize {
        let mut removed = 0;
        for sub in ["thumbs", "clips"] {
            let Ok(entries) = std::fs::read_dir(self.cache_dir.join(sub)) else { continue };
            for entry in entries.filter_map(Result::ok) {
                let name = entry.file_name().to_string_lossy().to_string();
                // Skip work in progress; it has no cache entry to judge yet.
                if name.contains(".partial.") {
                    continue;
                }
                let Some(stem) = name.rsplit_once('.').map(|(s, _)| s) else { continue };
                if !keep.contains(stem) && std::fs::remove_file(entry.path()).is_ok() {
                    removed += 1;
                }
            }
        }
        removed
    }
}

#[derive(Debug, Clone, Default)]
pub struct VideoInfo {
    pub codec: String,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub fps: Option<f64>,
}

/// ffprobe reports frame rate as a rational, e.g. `20/1` or `30000/1001`.
fn parse_frame_rate(raw: &str) -> Option<f64> {
    let (num, den) = raw.trim().split_once('/')?;
    let (num, den): (f64, f64) = (num.parse().ok()?, den.parse().ok()?);
    (den > 0.0 && num > 0.0).then_some(num / den)
}

pub struct PlayableClip {
    pub path: PathBuf,
    /// True when this is a transcode rather than the recording itself.
    pub transcoded: bool,
    pub codec: String,
}

/// Cache filenames are derived from event ids, which come from another
/// program's database — so they are sanitised rather than trusted.
fn safe_name(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

/// Confirm a clip path really is inside the backup directory.
///
/// The path comes from our own index, but it is *derived* from a path another
/// program wrote, so it is checked rather than trusted: a crafted entry must
/// not be able to make this serve `/etc/shadow`.
pub fn within(root: &Path, candidate: &Path) -> bool {
    let (Ok(root), Ok(candidate)) = (root.canonicalize(), candidate.canonicalize()) else {
        return false;
    };
    candidate.starts_with(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_rates_are_read_as_rationals() {
        assert_eq!(parse_frame_rate("20/1"), Some(20.0));
        // NTSC-style rates are not integers and must not be rounded away.
        let ntsc = parse_frame_rate("30000/1001").unwrap();
        assert!((ntsc - 29.97).abs() < 0.01, "{ntsc}");
        // A stream with no frame rate reports 0/0 rather than omitting it.
        assert_eq!(parse_frame_rate("0/0"), None);
        assert_eq!(parse_frame_rate("nonsense"), None);
    }

    #[test]
    fn browser_playability_is_decided_by_codec() {
        assert!(Media::browser_can_play("h264"));
        assert!(Media::browser_can_play("av1"));
        // The reason this whole module exists.
        assert!(!Media::browser_can_play("hevc"));
        assert!(!Media::browser_can_play(""));
    }

    #[test]
    fn scratch_files_keep_their_extension_last() {
        // ffmpeg chooses its muxer from the extension, so a name ending in
        // `.partial` makes it fail with "unable to choose an output format".
        let m = Media::new(std::env::temp_dir().join(format!("pm-part-{}", std::process::id())));
        let dest = m.thumb_path("abc");
        let tmp = m.partial(&dest, "jpg");
        assert!(tmp.to_string_lossy().ends_with(".partial.jpg"), "{tmp:?}");

        let clip = m.clip_path("abc");
        assert!(m.partial(&clip, "mp4").to_string_lossy().ends_with(".partial.mp4"));

        let _ = std::fs::remove_dir_all(m.cache_dir);
    }

    #[test]
    fn cache_names_cannot_escape_the_cache_directory() {
        assert_eq!(safe_name("../../etc/passwd"), "______etc_passwd");
        assert_eq!(safe_name("6a6c4375-0317"), "6a6c4375-0317");
    }

    #[test]
    fn paths_outside_the_backup_root_are_rejected() {
        let root = std::env::temp_dir().join(format!("pm-within-{}", std::process::id()));
        let inside = root.join("cam/day");
        std::fs::create_dir_all(&inside).unwrap();
        let file = inside.join("clip.mp4");
        std::fs::write(&file, b"x").unwrap();

        assert!(within(&root, &file));
        assert!(!within(&root, Path::new("/etc/hostname")));
        // A traversal that resolves outside must fail even though the string
        // starts inside.
        assert!(!within(&root, &root.join("../../etc/hostname")));

        std::fs::remove_dir_all(&root).unwrap();
    }
}
