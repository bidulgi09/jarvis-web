//! 마이크 녹음 (cpal). 누르고 있는 동안 녹음하고, 놓으면 16kHz 모노로 변환해 돌려준다.
//! cpal::Stream은 플랫폼에 따라 Send가 아니므로 전용 스레드 안에서만 만들고 버린다.

use anyhow::{anyhow, bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Sample, SampleFormat};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use tauri::{AppHandle, Emitter};

pub const WHISPER_RATE: u32 = 16_000;
const MAX_SECONDS: usize = 30;

type Captured = (Vec<f32>, u32);

pub struct Recording {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<Result<Captured>>>,
}

impl Recording {
    /// 기본 입력 장치로 녹음을 시작한다. 장치를 열지 못하면 바로 에러.
    pub fn start(app: AppHandle) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_t = stop.clone();
        let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();

        let handle = std::thread::spawn(move || -> Result<Captured> {
            let opened = open_stream();
            let (stream, buf, level, rate) = match opened {
                Ok(v) => {
                    let _ = ready_tx.send(Ok(()));
                    v
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(anyhow!("{e:#}")));
                    return Err(e);
                }
            };

            // 40ms마다 직전 구간의 피크 볼륨을 UI로 보낸다 (홀로그램 반응용)
            while !stop_t.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(40));
                let peak = f32::from_bits(level.swap(0, Ordering::Relaxed));
                let _ = app.emit("mic-level", peak.min(1.0));
            }
            drop(stream);
            let _ = app.emit("mic-level", 0.0f32);
            let samples = std::mem::take(&mut *buf.lock().unwrap());
            Ok((samples, rate))
        });

        ready_rx
            .recv()
            .context("녹음 스레드가 응답하지 않습니다")??;
        Ok(Self {
            stop,
            handle: Some(handle),
        })
    }

    /// 녹음을 멈추고 Whisper 입력 형식(16kHz, mono, f32)으로 돌려준다.
    pub fn finish(mut self) -> Result<Vec<f32>> {
        self.stop.store(true, Ordering::Relaxed);
        let handle = self
            .handle
            .take()
            .ok_or_else(|| anyhow!("이미 끝난 녹음"))?;
        let (samples, rate) = handle
            .join()
            .map_err(|_| anyhow!("녹음 스레드가 중단되었습니다"))??;
        Ok(resample_linear(&samples, rate, WHISPER_RATE))
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn open_stream() -> Result<(cpal::Stream, Arc<Mutex<Vec<f32>>>, Arc<AtomicU32>, u32)> {
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .context("마이크를 찾을 수 없습니다. 입력 장치를 연결해 주세요")?;
    let supported = device
        .default_input_config()
        .context("마이크 설정을 읽지 못했습니다")?;
    let rate = supported.sample_rate().0;
    let channels = supported.channels() as usize;
    let config = supported.config();

    let buf = Arc::new(Mutex::new(Vec::<f32>::with_capacity(rate as usize * 5)));
    let level = Arc::new(AtomicU32::new(0));
    let cap = rate as usize * MAX_SECONDS;

    let stream = match supported.sample_format() {
        SampleFormat::F32 => {
            build::<f32>(&device, &config, channels, cap, buf.clone(), level.clone())?
        }
        SampleFormat::I16 => {
            build::<i16>(&device, &config, channels, cap, buf.clone(), level.clone())?
        }
        SampleFormat::U16 => {
            build::<u16>(&device, &config, channels, cap, buf.clone(), level.clone())?
        }
        SampleFormat::I32 => {
            build::<i32>(&device, &config, channels, cap, buf.clone(), level.clone())?
        }
        other => bail!("지원하지 않는 마이크 샘플 형식: {other:?}"),
    };
    stream.play().context("녹음을 시작하지 못했습니다")?;
    Ok((stream, buf, level, rate))
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    cap: usize,
    buf: Arc<Mutex<Vec<f32>>>,
    level: Arc<AtomicU32>,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let stream = device.build_input_stream(
        config,
        move |data: &[T], _: &cpal::InputCallbackInfo| {
            let mut peak = 0f32;
            let Ok(mut b) = buf.lock() else { return };
            for frame in data.chunks(channels.max(1)) {
                // 채널 평균으로 모노 다운믹스
                let m =
                    frame.iter().map(|&s| s.to_sample::<f32>()).sum::<f32>() / frame.len() as f32;
                peak = peak.max(m.abs());
                if b.len() < cap {
                    b.push(m);
                }
            }
            let prev = f32::from_bits(level.load(Ordering::Relaxed));
            if peak > prev {
                level.store(peak.to_bits(), Ordering::Relaxed);
            }
        },
        |e| eprintln!("[audio] stream error: {e}"),
        None,
    )?;
    Ok(stream)
}

/// 선형 보간 리샘플링. 음성 인식용으로는 충분하다.
fn resample_linear(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    let ratio = from as f64 / to as f64;
    let out_len = (input.len() as f64 / ratio).floor() as usize;
    (0..out_len)
        .map(|i| {
            let pos = i as f64 * ratio;
            let i0 = pos.floor() as usize;
            let i1 = (i0 + 1).min(input.len() - 1);
            let t = (pos - i0 as f64) as f32;
            input[i0] * (1.0 - t) + input[i1] * t
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::resample_linear;

    #[test]
    fn resample_halves_length() {
        let v: Vec<f32> = (0..48_000).map(|i| (i as f32 * 0.01).sin()).collect();
        let out = resample_linear(&v, 48_000, 16_000);
        assert_eq!(out.len(), 16_000);
    }
}
