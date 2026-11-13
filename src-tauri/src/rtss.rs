//! Frame rate from RivaTuner Statistics Server shared memory "RTSSSharedMemoryV2".
//! Layout: RTSS SDK RTSSSharedMemory.h (v2 application entry's stable prefix).
//! https://github.com/jaxFF/RTSSTweaks/blob/master/thirdparty/rtss-sdk/Include/RTSSSharedMemory.h

/// Frame rate of one application hooked by RTSS.
#[derive(Debug, Clone, PartialEq)]
pub struct RtssFps {
    pub fps: f32,
    /// executable file name, e.g. "cs2.exe"
    pub app: String,
    pub pid: u32,
}

const HEADER_SIZE: usize = 20;
const ENTRY_SIZE: usize = 284;

fn dword(buf: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(buf.get(offset..offset.checked_add(4)?)?.try_into().ok()?))
}

/// Validate layout before either copying shared memory or indexing a snapshot.
fn layout(header: &[u8]) -> Option<(usize, usize, usize, usize)> {
    if dword(header, 0)? != 0x5254_5353 || dword(header, 4)? < 0x0002_0000 { return None; }
    let stride = dword(header, 8)? as usize;
    let start = dword(header, 12)? as usize;
    let count = dword(header, 16)? as usize;
    if stride < ENTRY_SIZE || start < HEADER_SIZE { return None; }
    let end = start.checked_add(stride.checked_mul(count)?)?;
    Some((stride, start, count, end))
}

fn parse(buf: &[u8], pid_hint: Option<u32>, now_ms: u32) -> Option<RtssFps> {
    let (stride, start, count, end) = layout(buf)?;
    buf.get(..end)?;
    let mut best: Option<(bool, u32, RtssFps)> = None;
    for i in 0..count {
        let offset = start.checked_add(i.checked_mul(stride)?)?;
        let entry = buf.get(offset..offset.checked_add(ENTRY_SIZE)?)?;
        let pid = dword(entry, 0)?;
        let time0 = dword(entry, 268)?;
        let time1 = dword(entry, 272)?;
        let frames = dword(entry, 276)?;
        let age = now_ms.wrapping_sub(time1);
        let elapsed = time1.wrapping_sub(time0);
        // RTSS documents zero time0 as uninitialised. Reject reversed/torn periods too.
        if pid == 0 || time0 == 0 || age > 2000 || elapsed == 0 || elapsed > i32::MAX as u32 { continue; }
        let preferred = Some(pid) == pid_hint;
        if best.as_ref().is_some_and(|(hint, recent, _)| (*hint && !preferred) || (*hint == preferred && *recent <= age)) { continue; }
        let name = &entry[4..264];
        let name = String::from_utf8_lossy(&name[..name.iter().position(|b| *b == 0).unwrap_or(name.len())]);
        let app = name.rsplit(['\\', '/']).next().unwrap_or_default().to_string();
        best = Some((preferred, age, RtssFps { fps: 1000.0 * frames as f32 / elapsed as f32, app, pid }));
    }
    best.map(|(_, _, sample)| sample)
}

/// Prefer the foreground application's fresh entry; otherwise use the freshest entry.
/// No OSD slot is claimed or changed: RTSS can keep its overlay disabled.
pub fn read_fps(pid_hint: Option<u32>) -> Option<RtssFps> {
    use windows::core::w;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Memory::{MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, VirtualQuery, FILE_MAP_READ, MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_GUARD, PAGE_NOACCESS};
    use windows::Win32::System::SystemInformation::GetTickCount;
    // Keep ownership of the read-only mapping for the whole snapshot. No references
    // into shared memory escape; RTSS may update it concurrently, so read volatile.
    unsafe {
        let handle = OpenFileMappingW(FILE_MAP_READ.0, false, w!("RTSSSharedMemoryV2")).ok()?;
        let view = MapViewOfFile(handle, FILE_MAP_READ, 0, 0, 0);
        let result = (|| {
            if view.Value.is_null() { return None; }
            let mut info = MEMORY_BASIC_INFORMATION::default();
            if VirtualQuery(Some(view.Value), &mut info, std::mem::size_of_val(&info)) == 0 || info.RegionSize < HEADER_SIZE
                || info.State != MEM_COMMIT || info.Protect.0 & (PAGE_GUARD.0 | PAGE_NOACCESS.0) != 0 { return None; }
            let ptr = view.Value.cast::<u8>();
            let header: Vec<u8> = (0..HEADER_SIZE).map(|i| ptr.add(i).read_volatile()).collect();
            let (stride, start, count, end) = layout(&header)?;
            // Bound the work even if a foreign process supplied a corrupt header.
            if end > info.RegionSize || count > 4096 { return None; }
            // Compact snapshot: RTSS entries are many KB (frame-time buffers, OSD), only the
            // first ENTRY_SIZE bytes of each matter, so copy those behind a rewritten header.
            let mut snapshot = header;
            snapshot[8..12].copy_from_slice(&(ENTRY_SIZE as u32).to_le_bytes());
            snapshot[12..16].copy_from_slice(&(HEADER_SIZE as u32).to_le_bytes());
            snapshot.reserve(count * ENTRY_SIZE);
            for i in 0..count {
                let entry = ptr.add(start + i * stride);
                snapshot.extend((0..ENTRY_SIZE).map(|j| entry.add(j).read_volatile()));
            }
            parse(&snapshot, pid_hint, GetTickCount())
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
    fn fixture(entries: &[(u32, u32, u32, u32)]) -> Vec<u8> {
        // Larger strides and offsets exercise forward-compatible layout handling.
        let mut buf = vec![0; 64 + 320 * entries.len()];
        put(&mut buf, 0, 0x5254_5353); put(&mut buf, 4, 0x0002_0014);
        put(&mut buf, 8, 320); put(&mut buf, 12, 64); put(&mut buf, 16, entries.len() as u32);
        for (i, &(pid, t0, t1, frames)) in entries.iter().enumerate() {
            let o = 64 + 320 * i;
            put(&mut buf, o, pid);
            let name = format!("C:\\Games\\game{pid}.exe");
            buf[o + 4..o + 4 + name.len()].copy_from_slice(name.as_bytes());
            put(&mut buf, o + 268, t0); put(&mut buf, o + 272, t1); put(&mut buf, o + 276, frames);
        }
        buf
    }
    #[test]
    fn preferred_pid_otherwise_freshest_and_file_name() {
        let buf = fixture(&[(11, 1000, 2000, 60), (22, 1500, 2500, 144)]);
        assert_eq!(parse(&buf, Some(11), 2600), Some(RtssFps { fps: 60.0, app: "game11.exe".into(), pid: 11 }));
        assert_eq!(parse(&buf, None, 2600).unwrap().pid, 22);
        assert_eq!(parse(&buf, Some(99), 2600).unwrap().fps, 144.0);
    }
    #[test]
    fn ignores_empty_stale_uninitialised_and_zero_or_reversed_periods() {
        let buf = fixture(&[(0, 3000, 4000, 1), (1, 1000, 1999, 60), (2, 0, 3900, 1),
            (3, 3900, 3900, 1), (4, 4000, 3900, 1), (5, 2000, 3000, 120)]);
        assert_eq!(parse(&buf, Some(1), 4000).unwrap().pid, 5);
        assert!(parse(&fixture(&[(1, 1, 1000, 1)]), None, 3001).is_none());
        assert!(parse(&fixture(&[(1, 1, 1000, 1)]), None, 3000).is_some());
    }
    #[test]
    fn tick_wrap_preserves_freshness_order_and_measurement() {
        let buf = fixture(&[(1, u32::MAX - 1500, u32::MAX - 500, 60), (2, u32::MAX - 899, 100, 120)]);
        assert_eq!(parse(&buf, None, 200).unwrap().pid, 2);
        assert_eq!(parse(&buf, Some(1), 200).unwrap().fps, 60.0);
        assert_eq!(parse(&buf, Some(2), 200).unwrap().fps, 120.0);
    }
    #[test]
    fn corrupt_headers_and_every_truncation_are_safe() {
        let good = fixture(&[(1, 1000, 2000, 60)]);
        for len in 0..good.len() { assert!(parse(&good[..len], None, 2100).is_none()); }
        for (offset, value) in [(0, 0), (4, 0x10000), (8, 0), (8, 280), (12, 0), (12, u32::MAX), (16, u32::MAX)] {
            let mut buf = good.clone(); put(&mut buf, offset, value);
            assert!(parse(&buf, None, 2100).is_none(), "{offset}: {value}");
        }
        assert!(parse(&fixture(&[]), None, 1).is_none());
    }
    #[test]
    fn unterminated_and_non_utf8_names_are_bounded() {
        let mut buf = fixture(&[(1, 1000, 2000, 0)]);
        buf[68..328].fill(0xff);
        let sample = parse(&buf, None, 2100).unwrap();
        assert_eq!(sample.fps, 0.0);
        assert_eq!(sample.app.chars().count(), 260);
    }
}

#[cfg(test)]
mod live {
    /// Dumps the live RTSS shared memory; run with `cargo test rtss_live_dump -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn rtss_live_dump() {
        use windows::core::w;
        use windows::Win32::System::Memory::{MapViewOfFile, OpenFileMappingW, FILE_MAP_READ};
        use windows::Win32::System::SystemInformation::GetTickCount;
        unsafe {
            let handle = OpenFileMappingW(FILE_MAP_READ.0, false, w!("RTSSSharedMemoryV2")).expect("mapping");
            let view = MapViewOfFile(handle, FILE_MAP_READ, 0, 0, 0);
            let p = view.Value.cast::<u8>();
            let d = |o: usize| u32::from_le_bytes([*p.add(o), *p.add(o + 1), *p.add(o + 2), *p.add(o + 3)]);
            println!("sig {:#x} ver {:#x} entry {} off {} count {} now {}", d(0), d(4), d(8), d(12), d(16), GetTickCount());
            let (stride, start, count) = (d(8) as usize, d(12) as usize, d(16) as usize);
            for i in 0..count {
                let o = start + i * stride;
                if d(o) == 0 { continue; }
                let name: Vec<u8> = (0..260).map(|j| *p.add(o + 4 + j)).take_while(|b| *b != 0).collect();
                println!("#{i} pid {} {} flags {:#x} t0 {} t1 {} frames {} frametime {}", d(o), String::from_utf8_lossy(&name), d(o + 264), d(o + 268), d(o + 272), d(o + 276), d(o + 280));
            }
            println!("read_fps(None) = {:?}", super::read_fps(None));
        }
    }
}
