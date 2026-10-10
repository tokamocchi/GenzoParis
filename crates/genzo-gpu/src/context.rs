//! GPU の初期化（ヘッドレス）と、デバイス・キューの扱い（docs/04_architecture.md の 1.3 節「GPU
//! スレッド（1 本）が wgpu のデバイスとキューを専有する」、6.3 節「GPU のエラー」）。
//!
//! # アダプターの選び方
//!
//! [`GpuContextOptions::from_env`] は次の環境変数を読む（どれも省略できる）:
//!
//! | 環境変数 | 内容 | 既定 |
//! |---|---|---|
//! | `GENZO_GPU` | `0`・`off`・`false` なら GPU を使わない（[`GpuContext::new`] が `None`） | 使う |
//! | `WGPU_BACKEND` | 使うバックエンド（`vulkan`・`metal`・`dx12`・`gl` のカンマ区切り。wgpu の書式） | Vulkan・Metal・DX12 |
//! | `WGPU_ADAPTER_NAME` | アダプターの名前に含まれる文字列（大文字・小文字を区別しない） | 指定なし |
//! | `WGPU_POWER_PREF` | `high`・`low`・`none` | `high` |
//! | `GENZO_GPU_ALLOW_SOFTWARE` | `0`・`off`・`false` ならソフトウェアの実装（Mesa の llvmpipe・WARP など）を使わない | 使う |
//!
//! アダプターが見つからない環境（GPU のない CI など）では [`GpuContext::new`] が `Ok(None)` を返す
//! （呼び出し側は CPU 版で処理する。04 の 1.4 節）。任意機能（shader-f16 など）は要求しない。
//!
//! # エラーの扱い
//!
//! - wgpu の「捕まえていないエラーでパニックする」既定の動作を、記録だけする処理に置き換える。
//!   資源の作成と投入はエラーのスコープで囲み、検証エラー・メモリ不足を [`GpuError`] で返す
//!   （[`GpuContext::scoped`]）。
//! - デバイスの消失はコールバックで記録し、以後の処理は [`GpuError::DeviceLost`] になる。初期化し
//!   直す（新しい [`GpuContext`] を作る）か、CPU 版に切り替えるかは呼び出し側が決める（6.3 節）。
//! - 投入した処理の完了は [`GpuContextOptions::wait_timeout`] まで待つ（ハングの検出）。
//!
//! # 性能の数値について
//!
//! この環境（Linux のコンテナ）の GPU は Mesa の llvmpipe（Vulkan のソフトウェア実装。CPU で
//! 動く）で、**性能の判断には使えない**。性能（PERF-01・PERF-10）は実機（M1・RTX 3080）の PoC-3 で
//! 測る。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use wgpu::util::DeviceExt;

use crate::error::{GpuError, Result};

/// 投入した処理の完了を待つ時間の既定値。**仮置き**: llvmpipe でフル解像度のタイルを処理しても
/// 十分に収まり、実機のハングは検出できる長さとして置いた。PoC-3 で実機の処理時間を見て決める。
pub const DEFAULT_WAIT_TIMEOUT: Duration = Duration::from_secs(60);

/// 記録しておく「捕まえていないエラー」の件数（古いものから捨てる）。
const MAX_UNCAPTURED: usize = 16;

/// アダプターの選び方（モジュールの doc の表）。
#[derive(Debug, Clone, PartialEq)]
pub struct GpuContextOptions {
    /// GPU を使うか（`false` なら [`GpuContext::new`] は `None`）。
    pub enabled: bool,
    /// 使うバックエンド。
    pub backends: wgpu::Backends,
    /// 電力の優先度。
    pub power_preference: wgpu::PowerPreference,
    /// アダプターの名前に含まれる文字列（大文字・小文字を区別しない）。`None` なら wgpu に選ばせる。
    pub adapter_name: Option<String>,
    /// ソフトウェアの実装（llvmpipe・WARP など）を使ってよいか。
    pub allow_software: bool,
    /// 投入した処理の完了を待つ時間。
    pub wait_timeout: Duration,
}

impl Default for GpuContextOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            backends: wgpu::Backends::PRIMARY,
            power_preference: wgpu::PowerPreference::HighPerformance,
            adapter_name: None,
            allow_software: true,
            wait_timeout: DEFAULT_WAIT_TIMEOUT,
        }
    }
}

/// `0`・`off`・`false`（大文字・小文字を区別しない）なら `false`。
fn env_flag(name: &str) -> Option<bool> {
    let v = std::env::var(name).ok()?;
    let v = v.trim().to_ascii_lowercase();
    Some(!matches!(v.as_str(), "0" | "off" | "false" | "no"))
}

impl GpuContextOptions {
    /// 環境変数から作る（モジュールの doc の表。指定がなければ既定値）。
    pub fn from_env() -> Self {
        let d = Self::default();
        Self {
            enabled: env_flag("GENZO_GPU").unwrap_or(d.enabled),
            backends: wgpu::Backends::from_env().unwrap_or(d.backends),
            power_preference: wgpu::PowerPreference::from_env().unwrap_or(d.power_preference),
            adapter_name: std::env::var("WGPU_ADAPTER_NAME")
                .ok()
                .filter(|s| !s.trim().is_empty()),
            allow_software: env_flag("GENZO_GPU_ALLOW_SOFTWARE").unwrap_or(d.allow_software),
            wait_timeout: d.wait_timeout,
        }
    }
}

/// 使っているアダプターの情報（ログ・計測の記録用。05 の 1.8 節「環境」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuAdapterSummary {
    /// アダプターの名前（例: `llvmpipe (LLVM 17.0.6, 256 bits)`）。
    pub name: String,
    /// バックエンド（例: `Vulkan`）。
    pub backend: String,
    /// 種類（例: `Cpu`・`DiscreteGpu`）。
    pub device_type: String,
    /// ドライバ。
    pub driver: String,
    /// ドライバの情報（版など）。
    pub driver_info: String,
    /// ソフトウェアの実装（CPU で動く）か。性能の数値は判断に使えない。
    pub is_software: bool,
}

impl std::fmt::Display for GpuAdapterSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}（{}、{}、{} {}）",
            self.name, self.backend, self.device_type, self.driver, self.driver_info
        )?;
        if self.is_software {
            f.write_str(" ※ソフトウェアの実装")?;
        }
        Ok(())
    }
}

impl GpuAdapterSummary {
    fn from_info(info: &wgpu::AdapterInfo) -> Self {
        Self {
            name: info.name.clone(),
            backend: format!("{:?}", info.backend),
            device_type: format!("{:?}", info.device_type),
            driver: info.driver.clone(),
            driver_info: info.driver_info.clone(),
            is_software: info.device_type == wgpu::DeviceType::Cpu,
        }
    }
}

/// デバイスの状態（消失・捕まえていないエラー）。コールバックから書き込む。
#[derive(Debug, Default)]
struct Health {
    lost: AtomicBool,
    lost_reason: Mutex<Option<String>>,
    uncaptured: Mutex<Vec<String>>,
}

impl Health {
    fn mark_lost(&self, reason: String) {
        let mut r = self
            .lost_reason
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if r.is_none() {
            *r = Some(reason);
        }
        self.lost.store(true, Ordering::SeqCst);
    }

    fn push_uncaptured(&self, message: String) {
        let mut v = self
            .uncaptured
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if v.len() >= MAX_UNCAPTURED {
            v.remove(0);
        }
        v.push(message);
    }
}

/// wgpu のアダプター・デバイス・キュー（ヘッドレス。ウィンドウのサーフェスを持たない）。
///
/// 複数のスレッドから使える（wgpu のデバイスとキューは `Send + Sync`）が、04 の 1.3 節のとおり、
/// アプリでは GPU スレッド 1 本から使う想定。
pub struct GpuContext {
    _instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    summary: GpuAdapterSummary,
    limits: wgpu::Limits,
    health: Arc<Health>,
    wait_timeout: Duration,
}

impl std::fmt::Debug for GpuContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuContext")
            .field("adapter", &self.summary)
            .field("lost", &self.is_lost())
            .finish_non_exhaustive()
    }
}

impl GpuContext {
    /// 環境変数の設定（[`GpuContextOptions::from_env`]）で作る。
    pub fn from_env() -> Result<Option<Self>> {
        Self::new(&GpuContextOptions::from_env())
    }

    /// アダプター・デバイス・キューを作る。
    ///
    /// - GPU を使わない設定、アダプターが見つからない、ソフトウェアの実装を使わない設定でソフトウェアの
    ///   アダプターしかない場合は `Ok(None)`（CPU 版で処理する）。
    /// - アダプターはあるがデバイスを作れない場合は [`GpuError::RequestDevice`]。
    pub fn new(options: &GpuContextOptions) -> Result<Option<Self>> {
        if !options.enabled {
            return Ok(None);
        }
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = options.backends;
        desc.flags = wgpu::InstanceFlags::default().with_env();
        let instance = wgpu::Instance::new(desc);
        let adapter = match &options.adapter_name {
            Some(name) => {
                let name = name.to_lowercase();
                pollster::block_on(instance.enumerate_adapters(options.backends))
                    .into_iter()
                    .find(|a| a.get_info().name.to_lowercase().contains(&name))
            }
            None => pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: options.power_preference,
                force_fallback_adapter: false,
                compatible_surface: None,
                ..Default::default()
            }))
            .ok(),
        };
        let Some(adapter) = adapter else {
            return Ok(None);
        };
        let summary = GpuAdapterSummary::from_info(&adapter.get_info());
        if summary.is_software && !options.allow_software {
            return Ok(None);
        }
        // 上限はアダプターが対応する値をそのまま要求する（大きな画像のバッファのため）。
        // 任意機能は要求しない（2.3 節: shader-f16 などに依存しない）。
        let limits = adapter.limits();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("genzo-gpu"),
            required_features: wgpu::Features::empty(),
            required_limits: limits.clone(),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .map_err(|e| GpuError::RequestDevice(e.to_string()))?;
        let health = Arc::new(Health::default());
        {
            let h = Arc::clone(&health);
            device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| {
                h.push_uncaptured(e.to_string());
            }));
            let h = Arc::clone(&health);
            device.set_device_lost_callback(move |reason, message| {
                h.mark_lost(format!("{reason:?}: {message}"));
            });
        }
        let limits = device.limits();
        Ok(Some(Self {
            _instance: instance,
            adapter,
            device,
            queue,
            summary,
            limits,
            health,
            wait_timeout: options.wait_timeout,
        }))
    }

    /// アダプターの情報。
    pub fn summary(&self) -> &GpuAdapterSummary {
        &self.summary
    }

    /// wgpu のアダプター。
    pub fn adapter(&self) -> &wgpu::Adapter {
        &self.adapter
    }

    /// wgpu のデバイス。
    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// wgpu のキュー。
    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// デバイスの上限。
    pub fn limits(&self) -> &wgpu::Limits {
        &self.limits
    }

    /// 投入した処理の完了を待つ時間。
    pub fn wait_timeout(&self) -> Duration {
        self.wait_timeout
    }

    /// デバイスが失われたか。
    pub fn is_lost(&self) -> bool {
        self.health.lost.load(Ordering::SeqCst)
    }

    /// 捕まえていないエラーの記録（新しいものが後。最大 16 件）。
    pub fn uncaptured_errors(&self) -> Vec<String> {
        self.health
            .uncaptured
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// デバイスが失われていれば [`GpuError::DeviceLost`]。
    pub fn check_alive(&self) -> Result<()> {
        if self.is_lost() {
            let reason = self
                .health
                .lost_reason
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
                .unwrap_or_else(|| "理由は不明".to_owned());
            return Err(GpuError::DeviceLost(reason));
        }
        Ok(())
    }

    /// デバイスを破棄する（以後の処理は [`GpuError::DeviceLost`]）。終了処理と、デバイスの消失からの
    /// 復帰（6.3 節）を確かめるテストに使う。
    pub fn destroy(&self) {
        self.health
            .mark_lost("destroy が呼ばれた（Destroyed）".to_owned());
        self.device.destroy();
    }

    /// `f` をエラーのスコープ（検証・メモリ不足・内部エラー）で囲んで実行し、エラーがあれば返す。
    pub fn scoped<T>(&self, f: impl FnOnce() -> T) -> Result<T> {
        self.check_alive()?;
        let internal = self.device.push_error_scope(wgpu::ErrorFilter::Internal);
        let oom = self.device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
        let validation = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let value = f();
        // スコープは作った順の逆に閉じる。
        let v = pollster::block_on(validation.pop());
        let o = pollster::block_on(oom.pop());
        let i = pollster::block_on(internal.pop());
        self.check_alive()?;
        if let Some(e) = v.or(o).or(i) {
            return Err(e.into());
        }
        Ok(value)
    }

    /// 内容 `bytes` のストレージバッファ（読み書き・コピー元・コピー先）を作る。空なら 16 バイトの 0。
    pub fn create_storage_init(&self, label: &str, bytes: &[u8]) -> Result<wgpu::Buffer> {
        self.check_buffer_size(label, bytes.len() as u64)?;
        let padded;
        let contents = if bytes.len() < 16 || !bytes.len().is_multiple_of(4) {
            let mut v = bytes.to_vec();
            v.resize(bytes.len().max(16).next_multiple_of(4), 0);
            padded = v;
            &padded[..]
        } else {
            bytes
        };
        self.scoped(|| {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents,
                    usage: wgpu::BufferUsages::STORAGE
                        | wgpu::BufferUsages::COPY_SRC
                        | wgpu::BufferUsages::COPY_DST,
                })
        })
    }

    /// 大きさ `size` バイト（4 の倍数に切り上げ、16 バイト以上）の 0 で埋めたストレージバッファを作る。
    pub fn create_storage(&self, label: &str, size: u64) -> Result<wgpu::Buffer> {
        let size = size.max(16).next_multiple_of(4);
        self.check_buffer_size(label, size)?;
        self.scoped(|| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        })
    }

    /// バッファの大きさがデバイスの上限（ストレージバッファとして結び付けられる大きさ）以下か。
    pub fn check_buffer_size(&self, what: &str, bytes: u64) -> Result<()> {
        let limit = self
            .limits
            .max_storage_buffer_binding_size
            .min(self.limits.max_buffer_size);
        if bytes > limit {
            return Err(GpuError::TooLarge {
                what: what.to_owned(),
                bytes,
                limit,
            });
        }
        Ok(())
    }

    /// コマンドを投入し、完了まで待つ（[`wait_timeout`](Self::wait_timeout) まで）。
    pub fn submit_and_wait(&self, commands: wgpu::CommandBuffer) -> Result<()> {
        let index = self.scoped(|| self.queue.submit(Some(commands)))?;
        self.wait(index)
    }

    /// 投入済みの処理 `index` の完了を待つ。
    pub fn wait(&self, index: wgpu::SubmissionIndex) -> Result<()> {
        self.check_alive()?;
        match self.device.poll(wgpu::PollType::Wait {
            submission_index: Some(index),
            timeout: Some(self.wait_timeout),
        }) {
            Ok(_) => self.check_alive(),
            Err(wgpu::PollError::Timeout) => Err(GpuError::Timeout(self.wait_timeout)),
            Err(e) => {
                self.check_alive()?;
                Err(GpuError::Internal(e.to_string()))
            }
        }
    }

    /// バッファ `buffer` の先頭 `size` バイト（4 の倍数に切り上げて読む）を CPU に読み出す。
    pub fn read_buffer(&self, buffer: &wgpu::Buffer, size: u64) -> Result<Vec<u8>> {
        let padded = size.max(4).next_multiple_of(4);
        if padded > buffer.size() {
            return Err(GpuError::Validation(format!(
                "読み出す大きさ {padded} がバッファ {} を超えます",
                buffer.size()
            )));
        }
        let staging = self.scoped(|| {
            self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("genzo-gpu.readback"),
                size: padded,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        })?;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("genzo-gpu.readback"),
            });
        encoder.copy_buffer_to_buffer(buffer, 0, &staging, 0, padded);
        self.submit_and_wait(encoder.finish())?;
        let (tx, rx) = std::sync::mpsc::channel();
        staging.map_async(wgpu::MapMode::Read, .., move |r| {
            let _ = tx.send(r);
        });
        self.check_alive()?;
        match self.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(self.wait_timeout),
        }) {
            Ok(_) => {}
            Err(wgpu::PollError::Timeout) => return Err(GpuError::Timeout(self.wait_timeout)),
            Err(e) => return Err(GpuError::Internal(e.to_string())),
        }
        self.check_alive()?;
        match rx.recv_timeout(self.wait_timeout) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(GpuError::Map(e.to_string())),
            Err(_) => return Err(GpuError::Map("読み出しの完了が通知されない".to_owned())),
        }
        let data = {
            let view = staging
                .get_mapped_range(..)
                .map_err(|e| GpuError::Map(e.to_string()))?;
            view[..size as usize].to_vec()
        };
        staging.unmap();
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_options_give_no_context() {
        let o = GpuContextOptions {
            enabled: false,
            ..Default::default()
        };
        assert!(GpuContext::new(&o).unwrap().is_none());
    }

    #[test]
    fn summary_is_displayed() {
        let s = GpuAdapterSummary {
            name: "llvmpipe".into(),
            backend: "Vulkan".into(),
            device_type: "Cpu".into(),
            driver: "llvmpipe".into(),
            driver_info: "Mesa".into(),
            is_software: true,
        };
        let t = s.to_string();
        assert!(t.contains("llvmpipe") && t.contains("ソフトウェア"), "{t}");
    }
}
