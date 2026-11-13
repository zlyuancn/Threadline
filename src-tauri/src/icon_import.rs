//! 远程图标导入。
//!
//! WebView 里 `<img>` 能显示远程图，但 canvas 会因为跨域把画布污染，拿不到像素；
//! 所以下载交给 Agent（WinInet，走系统证书与代理），前端只负责缩放成小图并存进配置。
//!
//! 归属：**Agent 壳**。它不碰业务状态，只是一次性下载动作。

use base64::Engine;

/// 原始图标上限。允许到 4MB：上游可能给一张大图，前端会缩到 128px 再存；
/// 真正落盘的大小由前端 canvas 决定，不会把 4MB 塞进 tunnels.json。
const MAX_BYTES: usize = 4 * 1024 * 1024;

pub struct ImportedIcon {
    pub mime: String,
    pub base64: String,
}

pub fn download(url: &str) -> Result<ImportedIcon, String> {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("只支持 http(s) 图标地址".to_string());
    }
    let bytes = fetch(url)?;
    let mime = sniff(&bytes).ok_or_else(|| "下载到的内容不是图片".to_string())?;
    Ok(ImportedIcon {
        mime: mime.to_string(),
        base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
    })
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn fetch(url: &str) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::Networking::WinInet::{
        InternetCloseHandle, InternetOpenUrlW, InternetOpenW, InternetReadFile,
        INTERNET_FLAG_NO_CACHE_WRITE, INTERNET_FLAG_RELOAD,
    };

    let agent = wide("Threadline/0.1");
    let target = wide(url);

    unsafe {
        let session = InternetOpenW(agent.as_ptr(), 0, std::ptr::null(), std::ptr::null(), 0);
        if session.is_null() {
            return Err("无法初始化网络会话".to_string());
        }
        let request = InternetOpenUrlW(
            session,
            target.as_ptr(),
            std::ptr::null(),
            0,
            INTERNET_FLAG_RELOAD | INTERNET_FLAG_NO_CACHE_WRITE,
            0,
        );
        if request.is_null() {
            InternetCloseHandle(session);
            return Err("下载图标失败（地址不可达或被拒绝）".to_string());
        }

        let mut data = Vec::new();
        let mut buffer = [0u8; 8192];
        loop {
            let mut read = 0u32;
            let ok = InternetReadFile(
                request,
                buffer.as_mut_ptr().cast(),
                buffer.len() as u32,
                &mut read,
            );
            if ok == 0 || read == 0 {
                break;
            }
            data.extend_from_slice(&buffer[..read as usize]);
            if data.len() > MAX_BYTES {
                InternetCloseHandle(request);
                InternetCloseHandle(session);
                return Err("图标超过 4MB".to_string());
            }
        }

        InternetCloseHandle(request);
        InternetCloseHandle(session);

        if data.is_empty() {
            return Err("下载到的图标为空".to_string());
        }
        Ok(data)
    }
}

/// 只按文件头判断，不信任 URL 后缀或服务器给的 content-type。
fn sniff(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, 0x50, 0x4e, 0x47]) {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF8") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    if bytes.starts_with(&[0x00, 0x00, 0x01, 0x00]) {
        return Some("image/x-icon");
    }
    let head = &bytes[..bytes.len().min(512)];
    if String::from_utf8_lossy(head).contains("<svg") {
        return Some("image/svg+xml");
    }
    None
}
