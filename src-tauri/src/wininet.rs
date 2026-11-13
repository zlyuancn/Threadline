//! 拥有：WinINET 代理例外（`HKCU\...\Internet Settings\ProxyOverride`）的读写、
//! reconcile 落盘、生效广播，以及系统代理环境（启用 / PAC）的读取。
//! 提供：`sync`。
//! 允许调用：`tl_protocol::proxy_override`（纯函数）、`windows-sys`（Registry + WinInet）。
//!
//! 为什么放在 Agent 而不是 Core：开机自启档的 Core 是 SYSTEM，读写的是 `S-1-5-18` 的 HKCU，
//! 对登录用户完全无效。Agent 以登录用户身份运行，是唯一能生效的位置。

use std::ptr;

use tl_protocol::proxy_override;
use windows_sys::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows_sys::Win32::Networking::WinInet::{
    InternetSetOptionW, INTERNET_OPTION_REFRESH, INTERNET_OPTION_SETTINGS_CHANGED,
};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_READ, KEY_WRITE, REG_EXPAND_SZ, REG_SZ,
};

const INTERNET_SETTINGS: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";
const PROXY_OVERRIDE: &str = "ProxyOverride";
const PROXY_ENABLE: &str = "ProxyEnable";
const AUTO_CONFIG_URL: &str = "AutoConfigURL";

#[derive(Debug, serde::Serialize)]
pub struct ProxyReport {
    pub ok: bool,
    /// 实际写入的托管条目（Core 用它覆盖管理集）。
    pub applied: Vec<String>,
    /// 系统代理是否启用。
    pub enabled: Option<bool>,
    /// 是否使用 PAC 脚本——此时 `ProxyOverride` 不生效，界面要提示改 PAC。
    pub pac: Option<bool>,
    pub message: Option<String>,
}

pub fn sync(desired: &[String], managed: &[String]) -> ProxyReport {
    let key = match open_settings(KEY_READ | KEY_WRITE) {
        Ok(key) => key,
        Err(message) => return failure(message),
    };

    let (current, kind) = match read_value(key, PROXY_OVERRIDE) {
        Ok(Some((value, kind))) => (value, kind),
        Ok(None) => (String::new(), REG_SZ),
        Err(message) => {
            close(key);
            return failure(message);
        }
    };

    let next = match proxy_override::reconcile(&current, desired, managed) {
        Ok(next) => next,
        Err(message) => {
            close(key);
            return failure(message);
        }
    };

    // 写回保持原类型：有些安装包用 REG_EXPAND_SZ 存（虽然域名里不会有变量）。
    let write_kind = if kind == REG_EXPAND_SZ {
        REG_EXPAND_SZ
    } else {
        REG_SZ
    };
    if let Err(message) = write_value(key, PROXY_OVERRIDE, &next.join(";"), write_kind) {
        close(key);
        return failure(message);
    }

    let enabled = read_value(key, PROXY_ENABLE)
        .ok()
        .flatten()
        .map(|(value, _)| !value.trim().is_empty() && value.trim() != "0");
    let pac = read_value(key, AUTO_CONFIG_URL)
        .ok()
        .flatten()
        .map(|(value, _)| !value.trim().is_empty());

    close(key);
    broadcast();

    ProxyReport {
        ok: true,
        applied: desired.to_vec(),
        enabled,
        pac,
        message: None,
    }
}

fn failure(message: String) -> ProxyReport {
    ProxyReport {
        ok: false,
        applied: Vec::new(),
        enabled: None,
        pac: None,
        message: Some(message),
    }
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn open_settings(access: u32) -> Result<HKEY, String> {
    let sub = wide(INTERNET_SETTINGS);
    let mut key: HKEY = ptr::null_mut();
    let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, access, &mut key) };
    if status != ERROR_SUCCESS {
        return Err(format!("打开 Internet Settings 失败（Win32 {status}）"));
    }
    Ok(key)
}

fn close(key: HKEY) {
    unsafe {
        RegCloseKey(key);
    }
}

fn read_value(key: HKEY, name: &str) -> Result<Option<(String, u32)>, String> {
    let name = wide(name);
    let mut kind = 0u32;
    let mut len = 0u32;
    let status = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            ptr::null(),
            &mut kind,
            ptr::null_mut(),
            &mut len,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if status != ERROR_SUCCESS {
        return Err(format!("读取注册表值失败（Win32 {status}）"));
    }
    if len < 2 {
        return Ok(Some((String::new(), kind)));
    }

    let mut buffer = vec![0u8; len as usize];
    let status = unsafe {
        RegQueryValueExW(
            key,
            name.as_ptr(),
            ptr::null(),
            &mut kind,
            buffer.as_mut_ptr(),
            &mut len,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(format!("读取注册表值失败（Win32 {status}）"));
    }

    // REG_SZ / REG_EXPAND_SZ 都是 UTF-16LE，末位带 NUL。
    // `slice::as_chunks` 要 Rust 1.88，而本项目的 MSRV 是 1.80，只能继续用 chunks_exact。
    #[allow(clippy::chunks_exact_to_as_chunks)]
    let units: Vec<u16> = buffer
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    let text = String::from_utf16_lossy(&units);
    Ok(Some((text.trim_end_matches('\0').to_string(), kind)))
}

fn write_value(key: HKEY, name: &str, value: &str, kind: u32) -> Result<(), String> {
    let name = wide(name);
    let data: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = unsafe { std::slice::from_raw_parts(data.as_ptr().cast::<u8>(), data.len() * 2) };
    let status = unsafe {
        RegSetValueExW(
            key,
            name.as_ptr(),
            0,
            kind,
            bytes.as_ptr(),
            bytes.len() as u32,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(format!("写入 ProxyOverride 失败（Win32 {status}）"));
    }
    Ok(())
}

fn broadcast() {
    // 不广播的话，已运行的浏览器不会重新读取例外名单（WinINET 只认这次通知）。
    unsafe {
        InternetSetOptionW(
            ptr::null_mut(),
            INTERNET_OPTION_SETTINGS_CHANGED,
            ptr::null_mut(),
            0,
        );
        InternetSetOptionW(ptr::null_mut(), INTERNET_OPTION_REFRESH, ptr::null_mut(), 0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn current_override() -> String {
        let key = open_settings(KEY_READ).expect("open Internet Settings");
        let value = read_value(key, PROXY_OVERRIDE).expect("read ProxyOverride");
        close(key);
        value.map(|(text, _)| text).unwrap_or_default()
    }

    /// 真机验证注册表读写与还原。
    ///
    /// **默认跳过**：它会改当前用户的 `ProxyOverride`。只有显式设置
    /// `THREADLINE_TEST_HKCU=1` 时才跑（跑完还原原值）。
    #[test]
    fn sync_round_trips_user_proxy_override() {
        if std::env::var("THREADLINE_TEST_HKCU").as_deref() != Ok("1") {
            eprintln!("跳过：设置 THREADLINE_TEST_HKCU=1 才动真实注册表");
            return;
        }

        let original = current_override();
        let probe = "threadline-test.invalid".to_string();

        let report = sync(std::slice::from_ref(&probe), &[]);
        assert!(report.ok, "写入失败：{:?}", report.message);
        assert!(
            current_override().to_lowercase().contains(&probe),
            "写入后读不到测试条目"
        );

        let cleanup = sync(&[], std::slice::from_ref(&probe));
        assert!(cleanup.ok, "还原失败：{:?}", cleanup.message);
        assert_eq!(current_override(), original, "必须还原成原值");
    }
}
