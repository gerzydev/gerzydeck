//! CPU temperature from MSI Afterburner's read-only "MAHMSharedMemory".
//! Layout verified against MSI Afterburner/SDK/Include/MAHMSharedMemory.h.
//! Afterburner must run with CPU temperature enabled in Settings → Monitoring.

const HEADER_SIZE: usize = 32;
const ENTRY_SIZE: usize = 1324;
const CPU_TEMPERATURE: u32 = 0x80;

fn dword(buf: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(buf.get(offset..offset.checked_add(4)?)?.try_into().ok()?))
}

fn layout(header: &[u8]) -> Option<(usize, usize, usize, usize)> {
    header.get(..HEADER_SIZE)?;
    if dword(header, 0)? != 0x4d41_484d || dword(header, 4)? < 0x0002_0000 { return None; }
    let start = dword(header, 8)? as usize;
    let count = dword(header, 12)? as usize;
    let stride = dword(header, 16)? as usize;
    if start < HEADER_SIZE || stride < ENTRY_SIZE { return None; }
    let end = start.checked_add(count.checked_mul(stride)?)?;
    Some((start, count, stride, end))
}

fn cpu_name(name: &[u8]) -> bool {
    let end = name.iter().position(|b| *b == 0).unwrap_or(name.len());
    let name = String::from_utf8_lossy(&name[..end]).trim().to_ascii_lowercase();
    let Some(rest) = name.strip_prefix("cpu") else { return false };
    let Some(core) = rest.strip_suffix(" temperature") else { return false };
    core.is_empty() || core.bytes().all(|b| b.is_ascii_digit())
}

fn parse(buf: &[u8]) -> Option<f32> {
    let (start, count, stride, end) = layout(buf)?;
    buf.get(..end)?;
    let mut by_id: Option<f32> = None;
    let mut by_name: Option<f32> = None;
    for i in 0..count {
        let offset = start.checked_add(i.checked_mul(stride)?)?;
        let entry = buf.get(offset..offset.checked_add(ENTRY_SIZE)?)?;
        let value = f32::from_bits(dword(entry, 1300)?);
        if !value.is_finite() || value <= 0.0 || value > 150.0 { continue; }
        if dword(entry, 1320)? == CPU_TEMPERATURE {
            by_id = Some(by_id.map_or(value, |old| old.max(value)));
        } else if cpu_name(&entry[..260]) {
            by_name = Some(by_name.map_or(value, |old| old.max(value)));
        }
    }
    by_id.or(by_name)
}

/// Current CPU temperature in °C; None when Afterburner or its CPU sensor is unavailable.
pub fn read_cpu_temp() -> Option<f32> {
    use windows::core::w;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Memory::{MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, VirtualQuery, FILE_MAP_READ, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_GUARD, PAGE_NOACCESS};
    // Own the read-only view until the snapshot is complete; volatile reads avoid
    // creating Rust references into memory Afterburner updates concurrently.
    unsafe {
        let handle = OpenFileMappingW(FILE_MAP_READ.0, false, w!("MAHMSharedMemory")).ok()?;
        let view = MapViewOfFile(handle, FILE_MAP_READ, 0, 0, 0);
        let result = (|| {
            if view.Value.is_null() { return None; }
            let mut info = MEMORY_BASIC_INFORMATION::default();
            if VirtualQuery(Some(view.Value), &mut info, std::mem::size_of_val(&info)) == 0 || info.RegionSize < HEADER_SIZE
                || info.State != MEM_COMMIT || info.Protect.0 & (PAGE_GUARD.0 | PAGE_NOACCESS.0) != 0 { return None; }
            let ptr = view.Value.cast::<u8>();
            let header: Vec<u8> = (0..HEADER_SIZE).map(|i| ptr.add(i).read_volatile()).collect();
            let (start, count, stride, end) = layout(&header)?;
            if end > info.RegionSize || count > 4096 { return None; }
            // Compact layout; copy only names, values and source ids, not GPU records,
            // localized strings or unused fields in potentially larger future entries.
            let mut snapshot = vec![0; HEADER_SIZE + count * ENTRY_SIZE];
            snapshot[..HEADER_SIZE].copy_from_slice(&header);
            snapshot[8..12].copy_from_slice(&(HEADER_SIZE as u32).to_le_bytes());
            snapshot[16..20].copy_from_slice(&(ENTRY_SIZE as u32).to_le_bytes());
            for i in 0..count {
                let source = ptr.add(start + i * stride);
                let target = &mut snapshot[HEADER_SIZE + i * ENTRY_SIZE..HEADER_SIZE + (i + 1) * ENTRY_SIZE];
                for range in [0..260, 1300..1304, 1320..1324] {
                    for j in range { target[j] = source.add(j).read_volatile(); }
                }
            }
            parse(&snapshot)
        })();
        if !view.Value.is_null() { let _ = UnmapViewOfFile(view); }
        let _ = CloseHandle(handle);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn put(buf: &mut [u8], offset: usize, value: u32) { buf[offset..offset + 4].copy_from_slice(&value.to_le_bytes()); }
    fn fixture(entries: &[(&str, u32, f32)]) -> Vec<u8> {
        let mut buf = vec![0; 64 + entries.len() * 1400];
        put(&mut buf, 0, 0x4d41_484d); put(&mut buf, 4, 0x0002_0000);
        put(&mut buf, 8, 64); put(&mut buf, 12, entries.len() as u32); put(&mut buf, 16, 1400);
        for (i, &(name, id, value)) in entries.iter().enumerate() {
            let o = 64 + i * 1400;
            buf[o..o + name.len()].copy_from_slice(name.as_bytes());
            put(&mut buf, o + 1300, value.to_bits()); put(&mut buf, o + 1320, id);
        }
        buf
    }
    #[test]
    fn source_id_wins_over_fallback_name_and_gpu_temperature() {
        let buf = fixture(&[("GPU temperature", 0, 95.0), ("CPU temperature", u32::MAX, 90.0), ("Localized sensor", 0x80, 62.5)]);
        assert_eq!(parse(&buf), Some(62.5));
    }
    #[test]
    fn hottest_cpu_core_is_selected() {
        assert_eq!(parse(&fixture(&[("CPU1 temperature", 0x80, 58.0), ("CPU2 temperature", 0x80, 77.0), ("CPU temperature", 0x80, 65.0)])), Some(77.0));
    }
    #[test]
    fn name_fallback_is_bounded_case_insensitive_and_recognizes_cores() {
        assert_eq!(parse(&fixture(&[("CPU temperature", u32::MAX, 62.0), ("Cpu12 Temperature", u32::MAX, 69.0), ("CPU usage", 0x90, 99.0)])), Some(69.0));
        assert!(parse(&fixture(&[("GPU temperature", 0, 95.0), ("CPU power temperature", 1, 85.0)])).is_none());
        let mut buf = fixture(&[("CPU temperature", 0, 60.0)]);
        buf[64..324].fill(0xff);
        assert!(parse(&buf).is_none());
    }
    #[test]
    fn unavailable_and_implausible_readings_are_ignored() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::MAX, -1.0, 0.0, 150.1] {
            assert!(parse(&fixture(&[("CPU temperature", 0x80, value)])).is_none());
        }
        assert_eq!(parse(&fixture(&[("CPU temperature", 0x80, 150.0)])), Some(150.0));
        assert_eq!(parse(&fixture(&[("CPU1 temperature", 0x80, f32::MAX), ("CPU2 temperature", 0x80, 55.0)])), Some(55.0));
    }
    #[test]
    fn invalid_headers_dead_signature_and_all_truncations_are_safe() {
        let good = fixture(&[("CPU temperature", 0x80, 63.0)]);
        for len in 0..good.len() { assert!(parse(&good[..len]).is_none()); }
        for (offset, value) in [(0, 0), (0, 0xdead), (4, 0x10000), (8, 0), (8, u32::MAX), (12, u32::MAX), (16, 0), (16, 1320)] {
            let mut buf = good.clone(); put(&mut buf, offset, value);
            assert!(parse(&buf).is_none(), "{offset}: {value}");
        }
        assert!(parse(&fixture(&[])).is_none());
    }
}

#[cfg(test)]
mod live {
    /// Reads the running Afterburner; `cargo test afterburner_live -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn afterburner_live() {
        println!("cpu temp = {:?}", super::read_cpu_temp());
    }
}
