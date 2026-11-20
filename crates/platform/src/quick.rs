//! Toggle langsung untuk Quick Panel (tanpa membuka Settings).

use windows::Devices::Radios::{Radio, RadioState};
use windows::Networking::Connectivity::NetworkInformation;
use windows::Networking::NetworkOperators::{NetworkOperatorTetheringManager, TetheringOperationalState};

fn tethering() -> Option<NetworkOperatorTetheringManager> {
    let p = NetworkInformation::GetInternetConnectionProfile().ok()?;
    NetworkOperatorTetheringManager::CreateFromConnectionProfile(&p).ok()
}

/// Status Mobile hotspot (None kalau tidak tersedia, mis. tidak ada internet).
pub fn hotspot_on() -> Option<bool> {
    let m = tethering()?;
    Some(m.TetheringOperationalState().ok()? == TetheringOperationalState::On)
}

/// Nyalakan/matikan Mobile hotspot. Berjalan di thread lain; `done(ok)` dipanggil setelahnya.
pub fn set_hotspot(on: bool, done: impl FnOnce(bool) + Send + 'static) {
    std::thread::spawn(move || {
        let ok = (|| -> Option<bool> {
            let m = tethering()?;
            let r = if on { m.StartTetheringAsync().ok()?.join().ok()? } else { m.StopTetheringAsync().ok()?.join().ok()? };
            Some(r.Status().ok()?.0 == 0)
        })()
        .unwrap_or(false);
        done(ok);
    });
}

fn radios() -> Vec<Radio> {
    let Some(list) = Radio::GetRadiosAsync().ok().and_then(|o| o.join().ok()) else { return vec![] };
    (0..list.Size().unwrap_or(0)).filter_map(|i| list.GetAt(i).ok()).collect()
}

/// "Airplane mode" sederhana: aktif kalau semua radio (Wi-Fi, Bluetooth, seluler) mati.
pub fn airplane_on() -> Option<bool> {
    let r = radios();
    if r.is_empty() {
        return None;
    }
    Some(r.iter().all(|x| x.State().map(|s| s != RadioState::On).unwrap_or(true)))
}

/// Matikan semua radio (on=true) atau nyalakan lagi (on=false).
pub fn set_airplane(on: bool, done: impl FnOnce(bool) + Send + 'static) {
    std::thread::spawn(move || {
        let _ = Radio::RequestAccessAsync().ok().and_then(|o| o.join().ok());
        let want = if on { RadioState::Off } else { RadioState::On };
        let mut ok = true;
        for r in radios() {
            let res = r.SetStateAsync(want).ok().and_then(|o| o.join().ok());
            ok &= res.map(|a| a.0 == 1).unwrap_or(false);
        }
        done(ok);
    });
}
