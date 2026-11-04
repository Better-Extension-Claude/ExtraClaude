//! Session GitHub (nom du compte et jeton), gardée chiffrée pour le compte Windows (DPAPI)
//! dans `%LOCALAPPDATA%\BetterExt\session.dat`. Rien n'est lisible depuis un autre compte
//! Windows ni sur un autre PC.

use crate::github::Session;
use std::path::PathBuf;
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};

fn path() -> PathBuf {
    betterext::data_dir().join("session.dat")
}

fn crypt(data: &[u8], protect: bool) -> Option<Vec<u8>> {
    let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
    let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
    let ok = unsafe {
        if protect {
            CryptProtectData(&input, std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut output)
        } else {
            CryptUnprotectData(&input, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut output)
        }
    };
    if ok == 0 || output.pbData.is_null() {
        return None;
    }
    let bytes = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe { LocalFree(output.pbData as _) };
    Some(bytes)
}

pub fn load_session() -> Option<Session> {
    let data = std::fs::read(path()).ok()?;
    serde_json::from_slice(&crypt(&data, false)?).ok()
}

/// Enregistre la session, ou l'efface (`None`, déconnexion).
pub fn save_session(session: Option<&Session>) -> Result<(), String> {
    let Some(session) = session else {
        let _ = std::fs::remove_file(path());
        return Ok(());
    };
    let plain = serde_json::to_vec(session).map_err(|e| e.to_string())?;
    let data = crypt(&plain, true).ok_or("Chiffrement de la session impossible.")?;
    std::fs::create_dir_all(betterext::data_dir()).map_err(|e| e.to_string())?;
    std::fs::write(path(), data).map_err(|e| format!("Enregistrement de la session impossible : {e}"))?;
    // Les anciennes « clés d'accès » sont remplacées par la connexion.
    let _ = std::fs::remove_file(betterext::data_dir().join("keys.dat"));
    Ok(())
}
