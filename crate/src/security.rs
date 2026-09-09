//! Local transport security shared by MCP and the configuration UI.

#[cfg(unix)]
use std::fs;
#[cfg(unix)]
use std::io::Read;

const SESSION_ID_BYTES: usize = 16;

pub fn new_session_id() -> Result<String, String> {
    let mut bytes = [0u8; SESSION_ID_BYTES];
    fill_random(&mut bytes)?;
    let mut id = String::from("pb_");
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut id, "{byte:02x}");
    }
    Ok(id)
}


pub fn is_loopback_host(host: &str) -> bool {
    let host = host.trim().to_ascii_lowercase();
    host == "localhost"
        || host.starts_with("localhost:")
        || host == "127.0.0.1"
        || host.starts_with("127.0.0.1:")
        || host == "[::1]"
        || host.starts_with("[::1]:")
}

pub fn is_same_loopback_origin(origin: &str, host: &str) -> bool {
    if !is_loopback_host(host) {
        return false;
    }
    let origin = origin.trim().to_ascii_lowercase();
    let Some(authority) = origin.strip_prefix("http://") else {
        return false;
    };
    authority == host.trim().to_ascii_lowercase()
}


#[cfg(unix)]
fn fill_random(bytes: &mut [u8]) -> Result<(), String> {
    fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(bytes))
        .map_err(|e| format!("secure random: {e}"))
}

#[cfg(windows)]
fn fill_random(bytes: &mut [u8]) -> Result<(), String> {
    const BCRYPT_USE_SYSTEM_PREFERRED_RNG: u32 = 0x0000_0002;
    #[link(name = "bcrypt")]
    unsafe extern "system" {
        fn BCryptGenRandom(
            algorithm: *mut std::ffi::c_void,
            buffer: *mut u8,
            length: u32,
            flags: u32,
        ) -> i32;
    }
    let status = unsafe {
        BCryptGenRandom(
            std::ptr::null_mut(),
            bytes.as_mut_ptr(),
            bytes.len() as u32,
            BCRYPT_USE_SYSTEM_PREFERRED_RNG,
        )
    };
    if status >= 0 {
        Ok(())
    } else {
        Err(format!("BCryptGenRandom failed: 0x{:08x}", status as u32))
    }
}

#[cfg(not(any(unix, windows)))]
fn fill_random(_bytes: &mut [u8]) -> Result<(), String> {
    Err("secure random source is unsupported on this platform".into())
}

#[cfg(test)]
mod tests {
    use super::{is_loopback_host, is_same_loopback_origin};
    #[test]
    fn rejects_dns_rebinding_hostnames() {
        assert!(is_loopback_host("127.0.0.1:8787"));
        assert!(is_loopback_host("localhost:8787"));
        assert!(is_loopback_host("[::1]:8787"));
        assert!(!is_loopback_host("attacker.example:8787"));
        assert!(!is_loopback_host("127.0.0.1.attacker.example"));
    }

    #[test]
    fn ui_origin_must_match_exact_loopback_authority() {
        assert!(is_same_loopback_origin(
            "http://127.0.0.1:8787",
            "127.0.0.1:8787"
        ));
        assert!(!is_same_loopback_origin(
            "http://127.0.0.1:3000",
            "127.0.0.1:8787"
        ));
        assert!(!is_same_loopback_origin(
            "https://127.0.0.1:8787",
            "127.0.0.1:8787"
        ));
    }
}
