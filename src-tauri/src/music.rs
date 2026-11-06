//! 음악 연동 (데스크톱)
//! - Spotify 로그인: 기본 브라우저로 로그인 화면을 열고, 127.0.0.1:43821에서 한 번만 돌아오는 주소를 받는다.
//!   토큰 교환과 재생 제어는 화면(웹뷰)이 Web API로 직접 한다 (PKCE라 비밀 키가 필요 없다).
//! - 진짜 파형: 윈도우에서 스피커로 나가는 소리를 그대로 듣는(WASAPI 루프백) 스트림을 열어
//!   32개 주파수 대역의 세기를 30fps로 화면에 보낸다. 녹음하거나 저장하지 않는다.

use anyhow::{anyhow, bail, Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

pub const REDIRECT_PORT: u16 = 43821;

/// 로그인 화면에서 돌아온 주소의 쿼리 문자열("code=...&state=...")을 돌려준다
pub fn spotify_auth(url: &str) -> Result<String> {
    if !url.starts_with("https://accounts.spotify.com/") {
        bail!("Spotify 로그인 주소가 아닙니다");
    }
    let listener = TcpListener::bind(("127.0.0.1", REDIRECT_PORT))
        .with_context(|| format!("127.0.0.1:{REDIRECT_PORT} 포트를 열지 못했습니다"))?;
    listener.set_nonblocking(true)?;
    crate::tools::open_url(url)?;
    let deadline = Instant::now() + Duration::from_secs(180);
    loop {
        match listener.accept() {
            Ok((mut sock, _)) => {
                sock.set_nonblocking(false)?;
                let mut line = String::new();
                BufReader::new(&sock).read_line(&mut line)?;
                // "GET /callback?code=...&state=... HTTP/1.1"
                let path = line.split_whitespace().nth(1).unwrap_or("");
                let Some(query) = path.strip_prefix("/callback?") else {
                    let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
                    continue;
                };
                let body = "<!doctype html><meta charset=utf-8><title>J.A.R.V.I.S.</title>\
                    <body style=\"background:#04030c;color:#e8f6ff;font-family:sans-serif;display:grid;place-items:center;height:100vh;margin:0\">\
                    <p>Spotify 연결 완료. 이 창을 닫고 자비스로 돌아가세요.</p>";
                let _ = write!(
                    sock,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                return Ok(query.to_string());
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() > deadline {
                    bail!("3분 안에 로그인이 끝나지 않았습니다");
                }
                std::thread::sleep(Duration::from_millis(120));
            }
            Err(e) => return Err(e.into()),
        }
    }
}

const BANDS: usize = 32;
const WIN: usize = 2048;

pub struct Loopback {
    stop: Arc<AtomicBool>,
}

impl Drop for Loopback {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Loopback {
    #[cfg(target_os = "windows")]
    pub fn start(app: AppHandle) -> Result<Self> {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        use cpal::SampleFormat;

        let stop = Arc::new(AtomicBool::new(false));
        let stop_t = stop.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<()>>();
        std::thread::spawn(move || {
            let opened = (|| -> Result<(cpal::Stream, Arc<Mutex<Vec<f32>>>, u32)> {
                let host = cpal::default_host();
                let dev = host
                    .default_output_device()
                    .context("스피커 장치를 찾지 못했습니다")?;
                let cfg = dev.default_output_config()?;
                let ch = cfg.channels() as usize;
                let rate = cfg.sample_rate().0;
                let buf = Arc::new(Mutex::new(Vec::<f32>::with_capacity(WIN * 2)));
                let b2 = buf.clone();
                let push = move |mono: &mut dyn Iterator<Item = f32>| {
                    if let Ok(mut b) = b2.lock() {
                        b.extend(mono);
                        let n = b.len();
                        if n > WIN * 2 {
                            b.drain(..n - WIN);
                        }
                    }
                };
                let err = |e| eprintln!("루프백 오류: {e}");
                // WASAPI는 출력 장치로 입력 스트림을 만들면 루프백으로 동작한다
                let stream = match cfg.sample_format() {
                    SampleFormat::F32 => dev.build_input_stream(
                        &cfg.clone().into(),
                        move |d: &[f32], _: &_| {
                            push(&mut d.chunks(ch).map(|f| f.iter().sum::<f32>() / ch as f32))
                        },
                        err,
                        None,
                    )?,
                    SampleFormat::I16 => dev.build_input_stream(
                        &cfg.clone().into(),
                        move |d: &[i16], _: &_| {
                            push(&mut d.chunks(ch).map(|f| {
                                f.iter().map(|&s| s as f32 / 32768.0).sum::<f32>() / ch as f32
                            }))
                        },
                        err,
                        None,
                    )?,
                    other => bail!("지원하지 않는 출력 형식: {other:?}"),
                };
                stream.play()?;
                Ok((stream, buf, rate))
            })();
            let (stream, buf, rate) = match opened {
                Ok(v) => {
                    let _ = ready_tx.send(Ok(()));
                    v
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(anyhow!("{e:#}")));
                    return;
                }
            };
            let edges = band_edges(rate);
            while !stop_t.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(33));
                let frame: Vec<f32> = match buf.lock() {
                    Ok(b) if b.len() >= WIN => b[b.len() - WIN..].to_vec(),
                    _ => continue,
                };
                let _ = app.emit("music-spectrum", spectrum(&frame, &edges));
            }
            drop(stream);
            let _ = app.emit("music-spectrum", vec![0.0f32; BANDS]);
        });
        ready_rx
            .recv()
            .context("소리 듣기 스레드가 응답하지 않습니다")??;
        Ok(Self { stop })
    }

    #[cfg(not(target_os = "windows"))]
    pub fn start(_app: AppHandle) -> Result<Self> {
        bail!("스피커 소리를 직접 듣는 기능은 윈도우에서만 됩니다. 화면의 '진짜 파형' 버튼으로 소리 공유를 허락해 주세요")
    }
}

/// 40Hz~16kHz를 로그 간격 32칸으로 나눈 FFT 빈 경계
fn band_edges(rate: u32) -> Vec<(usize, usize)> {
    let hz_per_bin = rate as f32 / WIN as f32;
    let (lo, hi) = (40f32, 16_000f32.min(rate as f32 / 2.0 - 1.0));
    (0..BANDS)
        .map(|i| {
            let f0 = lo * (hi / lo).powf(i as f32 / BANDS as f32);
            let f1 = lo * (hi / lo).powf((i + 1) as f32 / BANDS as f32);
            let a = (f0 / hz_per_bin).floor().max(1.0) as usize;
            let b = ((f1 / hz_per_bin).ceil() as usize).max(a + 1);
            (a, b.min(WIN / 2))
        })
        .collect()
}

/// 해닝 창 + 대역별 DFT 세기 → 0..1 (dB 눈금)
pub fn spectrum(frame: &[f32], edges: &[(usize, usize)]) -> Vec<f32> {
    let n = frame.len();
    let w: Vec<f32> = frame
        .iter()
        .enumerate()
        .map(|(i, x)| x * (0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()))
        .collect();
    edges
        .iter()
        .map(|&(a, b)| {
            // 넓은 대역은 빈 몇 개만 골라 계산 (CPU를 아끼려고)
            let step = ((b - a) / 6).max(1);
            let mut peak = 0f32;
            let mut k = a;
            while k < b {
                // 괴르첼 알고리즘: 빈 하나의 세기를 삼각함수 한 번으로 구한다
                let c = 2.0 * (std::f32::consts::TAU * k as f32 / n as f32).cos();
                let (mut s1, mut s2) = (0f32, 0f32);
                for x in &w {
                    let s0 = x + c * s1 - s2;
                    s2 = s1;
                    s1 = s0;
                }
                let pow = (s1 * s1 + s2 * s2 - c * s1 * s2).max(0.0);
                peak = peak.max(pow.sqrt() / (n as f32 / 4.0));
                k += step;
            }
            let db = 20.0 * peak.max(1e-6).log10();
            ((db + 70.0) / 60.0).clamp(0.0, 1.0)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tone_lands_in_its_band() {
        let rate = 48_000;
        let edges = band_edges(rate);
        let f = 1000.0;
        let frame: Vec<f32> = (0..WIN)
            .map(|i| (std::f32::consts::TAU * f * i as f32 / rate as f32).sin() * 0.5)
            .collect();
        let s = spectrum(&frame, &edges);
        let top = s
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        let hz = rate as f32 / WIN as f32;
        let (a, b) = edges[top];
        assert!(
            (a as f32 * hz) <= f * 1.15 && (b as f32 * hz) >= f * 0.85,
            "band {top}"
        );
        assert!(s[top] > 0.8);
    }
}
