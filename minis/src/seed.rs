//! Step 7: hand the newly installed SideInstaller this pairing, as Side by Side
//! hands one to SideInstaller on another iPhone
//! (`SideBySideView.handOffToSideInstaller`): the file goes where SideInstaller
//! keeps its own (`PrivateStore.pairingFile`), a merged file from an older
//! pairing is deleted, and the file is marked as imported, so SideInstaller
//! connects with it from its first launch instead of pairing itself.
//!
//! house_arrest vends the whole container because the app is signed for
//! development; Application Support is outside Documents, and SideInstaller
//! only moves a pairing file out of Documents when it has none of its own.

use std::path::Path;

use idevice::afc::opcode::AfcFopenMode;
use idevice::afc::AfcClient;
use idevice::house_arrest::HouseArrestClient;
use idevice::RsdService;

use crate::fail::{chain, Fail, Result};
use crate::tunnel::Connection;
use crate::ui;

pub const PAIRING_PATH: &str = "/Library/Application Support/rp_pairing_file.plist";
const MERGED_PATH: &str = "/Library/Application Support/combined_pairing_file.plist";
/// `Engine.importedPairingNameKey`.
const IMPORTED_NAME_KEY: &str = "importedPairingFileName";
/// The name SideInstaller shows the file under, as it shows "SideBySide.plist"
/// for one Side by Side hands over.
const HANDED_OVER_NAME: &str = "siboot.plist";

pub async fn hand_over(
    connection: &mut Connection,
    bundle_id: &str,
    pairing_path: &Path,
) -> Result<()> {
    ui::activity("opening SideInstaller's container");
    let pairing = std::fs::read(pairing_path).map_err(|e| {
        Fail::new(
            format!("Couldn't read {}.", pairing_path.display()),
            e.to_string(),
        )
    })?;

    let house_arrest =
        HouseArrestClient::connect_rsd(&mut connection.adapter, &mut connection.handshake)
            .await
            .map_err(|e| Fail::new("Couldn't reach house_arrest on this iPhone.", chain(&e)))?;
    let mut afc = house_arrest.vend_container(bundle_id).await.map_err(|e| {
        Fail::new(
            format!("Couldn't open {bundle_id}'s container. Import the pairing file in SideInstaller instead: it's at ~/.siboot/rp_pairing_file.plist in Minis."),
            chain(&e),
        )
    })?;

    for dir in ["/Library", "/Library/Application Support"] {
        let _ = afc.mk_dir(dir).await;
    }
    ui::activity("writing the pairing file into it");
    let written = write_verified(&mut afc, PAIRING_PATH, &pairing).await?;
    ui::ok(&format!(
        "pairing file written into SideInstaller ({written} bytes, read back and checked)"
    ));

    // There's none on a fresh install; one from an older pairing would hand
    // other apps that pairing.
    let _ = afc.remove(MERGED_PATH).await;

    let prefs_path = format!("/Library/Preferences/{bundle_id}.plist");
    match mark_imported(&mut afc, &prefs_path).await {
        Ok(()) => ui::detail(&format!(
            "marked it as the imported file \"{HANDED_OVER_NAME}\" ({prefs_path})"
        )),
        Err(e) => {
            ui::warn(
                "Couldn't mark the pairing file as imported. SideInstaller still connects with it, \
                 but may offer to pair again.",
            );
            ui::detail(&e.raw);
        }
    }
    Ok(())
}

/// Set `importedPairingFileName` in SideInstaller's preferences, keeping what's
/// there. Written beside the file and renamed over it, so a broken upload can't
/// leave a half-written preferences file.
async fn mark_imported(afc: &mut AfcClient, prefs_path: &str) -> Result<()> {
    let mut prefs = match read_if_present(afc, prefs_path).await? {
        Some(bytes) if !bytes.is_empty() => plist::from_bytes::<plist::Dictionary>(&bytes)
            .map_err(|e| Fail::new("SideInstaller's preferences aren't a plist.", e.to_string()))?,
        _ => plist::Dictionary::new(),
    };
    prefs.insert(IMPORTED_NAME_KEY.into(), HANDED_OVER_NAME.into());
    let mut bytes = Vec::new();
    plist::to_writer_binary(&mut bytes, &prefs).map_err(|e| {
        Fail::new(
            "Couldn't encode SideInstaller's preferences.",
            e.to_string(),
        )
    })?;
    let _ = afc.mk_dir("/Library/Preferences").await;
    let staging = format!("{prefs_path}.siboot");
    write_verified(afc, &staging, &bytes).await?;
    afc.rename(staging, prefs_path)
        .await
        .map_err(|e| Fail::new("Couldn't save SideInstaller's preferences.", chain(&e)))?;
    Ok(())
}

// ---- siboot-mod: LiveContainer + SideStore ---------------------------------
//
// SideStore is LiveContainer's built-in guest: its home is `Documents/SideStore`
// in LiveContainer's container, as in SideInstaller (`PairingTargets.swift`,
// `SideStoreHome.liveContainer`). Each record goes in its own file, the whole
// file in the legacy one, and SideStore's preferences get the two settings its
// own import sets.

const LC_PAIRING_DIR: &str = "/Documents/SideStore/Documents";
const LC_PREFS_DIR: &str = "/Documents/SideStore/Library/Preferences";
const LC_PREFS: &str = "/Documents/SideStore/Library/Preferences/com.SideStore.SideStore.plist";
const LC_LEGACY_FILE: &str = "ALTPairingFile.mobiledevicepairing";
const LC_REMOTE_FILE: &str = "PairingFile_RemoteRP.plist";
const LC_LOCKDOWN_FILE: &str = "PairingFile_Lockdown.plist";
/// Every RPPairing key; the rest of a file is its lockdown record.
const REMOTE_KEYS: [&str; 4] = ["private_key", "public_key", "identifier", "alt_irk"];
const REMOTE_REQUIRED: [&str; 3] = ["private_key", "public_key", "identifier"];
const LOCKDOWN_REQUIRED: [&str; 10] = [
    "WiFiMACAddress",
    "SystemBUID",
    "RootPrivateKey",
    "HostPrivateKey",
    "HostID",
    "RootCertificate",
    "UDID",
    "EscrowBag",
    "HostCertificate",
    "DeviceCertificate",
];

pub async fn hand_over_livecontainer(
    connection: &mut Connection,
    bundle_id: &str,
    pairing_path: &Path,
) -> Result<()> {
    ui::activity("opening LiveContainer's container");
    let bytes = std::fs::read(pairing_path).map_err(|e| {
        Fail::new(
            format!("Couldn't read {}.", pairing_path.display()),
            e.to_string(),
        )
    })?;
    let pairing = plist::from_bytes::<plist::Dictionary>(&bytes)
        .map_err(|e| Fail::new("The pairing file isn't a plist.", e.to_string()))?;

    let mut remote = plist::Dictionary::new();
    let mut lockdown = plist::Dictionary::new();
    for (key, value) in pairing.iter() {
        if REMOTE_KEYS.contains(&key.as_str()) {
            remote.insert(key.clone(), value.clone());
        } else {
            lockdown.insert(key.clone(), value.clone());
        }
    }
    let has_remote = REMOTE_REQUIRED.iter().all(|k| remote.contains_key(*k));
    let has_lockdown = LOCKDOWN_REQUIRED.iter().all(|k| lockdown.contains_key(*k));
    if !has_remote && !has_lockdown {
        return Err(Fail::new(
            "The pairing file has no complete record to hand over.",
            "",
        ));
    }

    let house_arrest =
        HouseArrestClient::connect_rsd(&mut connection.adapter, &mut connection.handshake)
            .await
            .map_err(|e| Fail::new("Couldn't reach house_arrest on this iPhone.", chain(&e)))?;
    let mut afc = house_arrest.vend_container(bundle_id).await.map_err(|e| {
        Fail::new(
            format!("Couldn't open {bundle_id}'s container."),
            chain(&e),
        )
    })?;

    for dir in [
        "/Documents",
        "/Documents/SideStore",
        LC_PAIRING_DIR,
        "/Documents/SideStore/Library",
        LC_PREFS_DIR,
    ] {
        let _ = afc.mk_dir(dir).await;
    }
    ui::activity("writing the pairing file into LiveContainer's SideStore");
    let legacy = format!("{LC_PAIRING_DIR}/{LC_LEGACY_FILE}");
    let written = write_verified(&mut afc, &legacy, &xml(&pairing)?).await?;
    if has_remote {
        let path = format!("{LC_PAIRING_DIR}/{LC_REMOTE_FILE}");
        write_verified(&mut afc, &path, &xml(&remote)?).await?;
    }
    if has_lockdown {
        let path = format!("{LC_PAIRING_DIR}/{LC_LOCKDOWN_FILE}");
        write_verified(&mut afc, &path, &xml(&lockdown)?).await?;
    }
    ui::ok(&format!(
        "pairing file written into LiveContainer's SideStore ({written} bytes, read back and checked)"
    ));

    let protocol = if has_lockdown { "lockdown" } else { "rppairing" };
    let settings: [(&str, plist::Value); 2] = [
        ("isPairingReset", false.into()),
        ("activePairingProtocol", protocol.into()),
    ];
    match update_prefs(&mut afc, LC_PREFS, &settings).await {
        Ok(()) => ui::detail(&format!("set the imported pairing protocol to {protocol} ({LC_PREFS})")),
        Err(e) => {
            ui::warn(
                "Couldn't set SideStore's pairing settings. It may ask for the pairing file; \
                 import ~/.siboot/rp_pairing_file.plist from Minis in SideStore.",
            );
            ui::detail(&e.raw);
        }
    }
    Ok(())
}

fn xml(dict: &plist::Dictionary) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    plist::to_writer_xml(&mut out, dict)
        .map_err(|e| Fail::new("Couldn't encode the pairing file.", e.to_string()))?;
    Ok(out)
}

/// Merge `entries` into the plist at `path`, keeping what's there. Written
/// beside the file and renamed over it, like `mark_imported`.
async fn update_prefs(
    afc: &mut AfcClient,
    path: &str,
    entries: &[(&str, plist::Value)],
) -> Result<()> {
    let mut prefs = match read_if_present(afc, path).await? {
        Some(bytes) if !bytes.is_empty() => plist::from_bytes::<plist::Dictionary>(&bytes)
            .map_err(|e| Fail::new("SideStore's preferences aren't a plist.", e.to_string()))?,
        _ => plist::Dictionary::new(),
    };
    for (key, value) in entries {
        prefs.insert((*key).to_string(), value.clone());
    }
    let mut bytes = Vec::new();
    plist::to_writer_binary(&mut bytes, &prefs)
        .map_err(|e| Fail::new("Couldn't encode SideStore's preferences.", e.to_string()))?;
    let staging = format!("{path}.siboot");
    write_verified(afc, &staging, &bytes).await?;
    afc.rename(staging, path)
        .await
        .map_err(|e| Fail::new("Couldn't save SideStore's preferences.", chain(&e)))?;
    Ok(())
}

async fn read_if_present(afc: &mut AfcClient, path: &str) -> Result<Option<Vec<u8>>> {
    if afc.get_file_info(path).await.is_err() {
        return Ok(None);
    }
    let mut file = afc
        .open(path, AfcFopenMode::RdOnly)
        .await
        .map_err(|e| Fail::new(format!("Couldn't open {path}."), chain(&e)))?;
    let bytes = file
        .read_entire()
        .await
        .map_err(|e| Fail::new(format!("Couldn't read {path}."), chain(&e)))?;
    let _ = file.close().await;
    Ok(Some(bytes))
}

/// Write `data` to `path`, then read it back and compare. Returns its length.
async fn write_verified(afc: &mut AfcClient, path: &str, data: &[u8]) -> Result<usize> {
    let mut file = afc
        .open(path, AfcFopenMode::WrOnly)
        .await
        .map_err(|e| Fail::new(format!("Couldn't create {path}."), chain(&e)))?;
    file.write_entire(data)
        .await
        .map_err(|e| Fail::new(format!("Couldn't write {path}."), chain(&e)))?;
    file.close()
        .await
        .map_err(|e| Fail::new(format!("Couldn't write {path}."), chain(&e)))?;
    match read_if_present(afc, path).await? {
        Some(back) if back == data => Ok(data.len()),
        Some(back) => Err(Fail::new(
            format!("{path} reads back different from what was written."),
            format!("wrote {} bytes, read {}", data.len(), back.len()),
        )),
        None => Err(Fail::new(format!("{path} vanished after writing."), "")),
    }
}
