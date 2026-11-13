//! 拥有：机级对称加密的绑定（Windows DPAPI，machine scope）。
//! 提供：`seal` / `open`（字节进、字节出，不关心内容语义）。
//! 允许调用：`windows-sys`（`CryptProtectData` / `CryptUnprotectData` / `LocalFree`）。
//!
//! 为什么 machine scope（`CRYPTPROTECT_LOCAL_MACHINE`）：同一个凭据文件要能被**两种身份**解——
//! 以 SYSTEM 运行的核心（开机自启）与以用户身份运行的核心（双击启动、顶班）。用 current-user
//! scope 会让另一方永远解不开，从而被迫做 per-identity 分支。
//!
//! 安全边界：machine scope 的强度上限就是**文件 ACL**——能读到密文文件的本机主体就能解开。
//! 因此调用方必须把凭据文件的 ACL 收紧到「SYSTEM + Administrators + 交互用户」。

/// `CryptProtectData` 的 `CRYPTPROTECT_LOCAL_MACHINE` 标志。
const CRYPTPROTECT_LOCAL_MACHINE: u32 = 0x4;

#[cfg(windows)]
mod imp {
    use super::CRYPTPROTECT_LOCAL_MACHINE;
    use anyhow::{anyhow, Result};
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB,
    };

    fn last_error() -> std::io::Error {
        std::io::Error::last_os_error()
    }

    /// 把输出 blob 拷出来并释放系统内存。
    unsafe fn take_blob(blob: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        let bytes = if blob.pbData.is_null() {
            Vec::new()
        } else {
            std::slice::from_raw_parts(blob.pbData, blob.cbData as usize).to_vec()
        };
        if !blob.pbData.is_null() {
            LocalFree(blob.pbData as _);
        }
        bytes
    }

    pub fn seal(plaintext: &[u8]) -> Result<Vec<u8>> {
        let input = CRYPT_INTEGER_BLOB {
            cbData: plaintext.len() as u32,
            pbData: plaintext.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };

        let ok = unsafe {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_LOCAL_MACHINE,
                &mut output,
            )
        };
        if ok == 0 {
            return Err(anyhow!("CryptProtectData 失败：{}", last_error()));
        }
        Ok(unsafe { take_blob(output) })
    }

    pub fn open(sealed: &[u8]) -> Result<Vec<u8>> {
        let input = CRYPT_INTEGER_BLOB {
            cbData: sealed.len() as u32,
            pbData: sealed.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };

        let ok = unsafe {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                &mut output,
            )
        };
        if ok == 0 {
            return Err(anyhow!("CryptUnprotectData 失败：{}", last_error()));
        }
        Ok(unsafe { take_blob(output) })
    }
}

#[cfg(windows)]
pub use imp::{open, seal};

#[cfg(not(windows))]
compile_error!("Threadline 只支持 Windows（DPAPI 绑定依赖 Win32）");

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_recovers_plaintext() {
        let plaintext = b"-----BEGIN OPENSSH PRIVATE KEY-----\nsecret\n-----END-----\n";
        let sealed = seal(plaintext).expect("seal");
        assert_ne!(sealed.as_slice(), plaintext, "密文不能等于明文");
        assert_eq!(open(&sealed).expect("open"), plaintext);
    }

    #[test]
    fn sealed_blob_does_not_leak_plaintext() {
        let plaintext = b"correct-horse-battery-staple";
        let sealed = seal(plaintext).expect("seal");
        assert!(
            !sealed
                .windows(plaintext.len())
                .any(|w| w == plaintext.as_slice()),
            "密文中不得出现明文片段"
        );
    }

    #[test]
    fn tampered_blob_fails_to_open() {
        let mut sealed = seal(b"data").expect("seal");
        let last = sealed.len() - 1;
        sealed[last] ^= 0xFF;
        assert!(open(&sealed).is_err(), "篡改后的密文必须解密失败");
    }

    #[test]
    fn empty_payload_roundtrips() {
        let sealed = seal(b"").expect("seal empty");
        assert!(open(&sealed).expect("open empty").is_empty());
    }

    #[test]
    fn two_seals_of_same_input_differ() {
        // DPAPI 每次加密带随机盐：同样的明文不应产生同样的密文。
        let a = seal(b"same").expect("a");
        let b = seal(b"same").expect("b");
        assert_ne!(a, b);
    }
}
