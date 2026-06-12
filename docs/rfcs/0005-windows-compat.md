# RFC 0005: Windows Compatibility

**Status**: Draft  
**Author**: modeld Architecture Team  
**Created**: 2024  
**Last Updated**: 2024  
**Priority**: CRITICAL - Highest Risk Area

## Abstract

This RFC defines comprehensive Windows compatibility strategies for modeld, addressing filesystem limitations including symlink privilege requirements, cross-volume hardlink prohibitions, junction point semantics, and filesystem-specific constraints. The design provides a multi-tier fallback strategy with privilege detection, user communication guidelines, and comprehensive testing matrices to ensure modeld works reliably across all Windows configurations.

## Motivation

Windows imposes unique filesystem constraints that significantly impact content-addressable storage systems:

- **90% of users lack symlink privileges** by default (require Developer Mode or Administrator)
- **Cross-volume hardlinks are prohibited** by NTFS design (common multi-disk setups)
- **Junction points work differently** than Unix symlinks (directory-only, different semantics)
- **Multiple filesystem types** with varying capabilities (NTFS, ReFS, exFAT, FAT32)
- **User experience expectations** require transparent operation without manual privilege escalation

Without careful design, modeld would either:
1. Fail completely on standard Windows installations (bad UX)
2. Require Administrator privileges for all operations (security concern)
3. Provide degraded functionality without clear user communication (confusing UX)

This RFC provides a comprehensive strategy to handle all Windows scenarios gracefully.

## Problem Statement

Design Windows compatibility strategies that:

1. **Work without privileges** for basic operations (scanning, reporting duplicates)
2. **Degrade gracefully** when full functionality is unavailable
3. **Detect capabilities** at runtime without user intervention
4. **Provide clear guidance** when user action is needed (Developer Mode setup)
5. **Handle all filesystem types** (NTFS, ReFS, exFAT, FAT32)
6. **Support common scenarios** (same volume, cross volume, USB drives, network shares)
7. **Test comprehensively** to prevent platform-specific bugs

## Windows Filesystem Limitations Overview

### Critical Limitations

| Limitation | Details | Impact | Affected Users | Mitigation |
|------------|---------|--------|----------------|------------|
| **Symlink Privileges** | Creating symlinks requires SeCreateSymbolicLinkPrivilege (Developer Mode or Admin) | **HIGH** | ~90% of users | Privilege detection + Developer Mode instructions |
| **Cross-Volume Hardlinks** | Hardlinks cannot span volumes (C:\\ → D:\\ fails) | **HIGH** | Multi-disk setups (~60%) | Fallback to symlinks or reference-only |
| **Junction Points** | Directory-only, different from file symlinks | **MEDIUM** | All users | Limited use for virtual FS directories |
| **NTFS vs Others** | ReFS, exFAT, FAT32 have different capabilities | **MEDIUM** | USB drives, older systems | Filesystem detection + tailored strategies |
| **Path Length (260 chars)** | Legacy MAX_PATH limit unless opted in | **LOW** | Deep directory structures | Use long path prefix (\\\\?\\) |
| **Case Insensitivity** | NTFS is case-insensitive by default | **LOW** | Hash collisions unlikely | BLAKE3 hashes are lowercase hex |
| **File Locking** | More aggressive than Unix | **LOW** | Concurrent access | Retry logic + proper file handles |

### Detailed Analysis

#### 1. Symlink Privilege Requirements

**Background**:
Windows historically required Administrator privileges to create symbolic links to prevent security exploits. Windows 10 Build 14972+ introduced Developer Mode, which grants `SeCreateSymbolicLinkPrivilege` to standard users.

**Technical Details**:
```
Privilege Name: SeCreateSymbolicLinkPrivilege
Required for: CreateSymbolicLinkW() / CreateSymbolicLinkA() Win32 API
Default state: Not granted to standard users
Grant methods:
  1. Enable Developer Mode (Settings → For Developers)
  2. Run as Administrator (UAC prompt)
  3. Group Policy modification (enterprise environments)
```

**Detection**:
```rust
// Test symlink creation to detect privilege
use std::os::windows::fs::symlink_file;
use std::path::Path;

fn has_symlink_privilege() -> bool {
    let temp_dir = std::env::temp_dir();
    let test_target = temp_dir.join("modeld_symlink_test_target.txt");
    let test_link = temp_dir.join("modeld_symlink_test_link.txt");
    
    // Create target file
    std::fs::write(&test_target, "test").ok()?;
    
    // Attempt symlink creation
    let result = symlink_file(&test_target, &test_link);
    
    // Cleanup
    let _ = std::fs::remove_file(&test_link);
    let _ = std::fs::remove_file(&test_target);
    
    result.is_ok()
}
```

**Error Codes**:
- `ERROR_PRIVILEGE_NOT_HELD (1314)`: User lacks SeCreateSymbolicLinkPrivilege
- `ERROR_INVALID_FUNCTION (1)`: Filesystem doesn't support symlinks (FAT32, exFAT)

**Impact on modeld**:
- Without privileges: Cannot create cross-volume links
- With privileges: Full functionality (hardlink same-volume, symlink cross-volume)

**User Distribution** (estimated):
- Standard users (no Dev Mode): ~90%
- Developer Mode enabled: ~5-8%
- Running as Administrator: ~2-5%

#### 2. Cross-Volume Hardlink Prohibition

**Background**:
NTFS hardlinks are implemented at the filesystem level by sharing the same MFT (Master File Table) record. This is only possible within a single volume.

**Technical Details**:
```
Hardlink mechanism: Multiple directory entries → Same MFT record → Same on-disk data
Limitation: MFT records are volume-specific (cannot reference across C:\ and D:\)
API: CreateHardLinkW() returns ERROR_INVALID_FUNCTION (1) for cross-volume attempts
```

**Detection**:
```rust
use std::path::Path;

fn get_volume_path(path: &Path) -> std::io::Result<PathBuf> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use winapi::um::fileapi::GetVolumePathNameW;
        
        let wide_path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut volume_buf = vec![0u16; 260];
        
        unsafe {
            let result = GetVolumePathNameW(
                wide_path.as_ptr(),
                volume_buf.as_mut_ptr(),
                volume_buf.len() as u32
            );
            
            if result != 0 {
                let len = volume_buf.iter().position(|&c| c == 0).unwrap_or(0);
                let volume_str = String::from_utf16_lossy(&volume_buf[..len]);
                return Ok(PathBuf::from(volume_str));
            }
        }
    }
    
    Err(std::io::Error::last_os_error())
}

fn is_same_volume(path1: &Path, path2: &Path) -> bool {
    match (get_volume_path(path1), get_volume_path(path2)) {
        (Ok(vol1), Ok(vol2)) => vol1 == vol2,
        _ => false,  // Assume different volumes on error (conservative)
    }
}
```

**Common Scenarios**:
- User has ComfyUI on C:\\ and modeld store on D:\\ → Cross-volume (cannot hardlink)
- Multiple model directories across C:\\, D:\\, E:\\ → Mixed scenarios
- External USB drive models → Almost always cross-volume

**Impact on modeld**:
- Same volume: Hardlinks work perfectly (zero space overhead)
- Cross volume: Must use symlinks (requires privileges) or reference-only mode

#### 3. Junction Points vs Symlinks

**Junction Points**:
- **Type**: NTFS reparse points (predates Vista symlinks)
- **Scope**: Directories only (cannot link files)
- **Privileges**: No special privileges required (works for all users)
- **API**: `CreateSymbolicLinkW()` with `SYMBOLIC_LINK_FLAG_DIRECTORY` flag (not a separate API)
- **Limitation**: Absolute paths only (no relative junctions), local volumes only (no UNC)

**Symlinks**:
- **Type**: Modern symbolic links (Vista+)
- **Scope**: Files and directories
- **Privileges**: Requires SeCreateSymbolicLinkPrivilege (Developer Mode or Admin)
- **API**: `CreateSymbolicLinkW()` / `CreateSymbolicLinkA()`
- **Features**: Supports relative paths, can span UNC paths

**Comparison**:

| Feature | Junction Point | Symlink |
|---------|---------------|---------|
| Target type | Directories only | Files + Directories |
| Privilege required | ✗ None | ✓ SeCreateSymbolicLink |
| Path type | Absolute only | Relative + Absolute |
| UNC paths | ✗ Not supported | ✓ Supported |
| Works for all users | ✓ Yes | ✗ No (needs Dev Mode) |
| Created via | CreateSymbolicLinkW + flag | CreateSymbolicLinkW |

**Use Cases in modeld**:

**Junction Points**:
- Virtual FS directory mounting: `D:\ComfyUI\models\` → `C:\modeld\virtual\comfuui\`
- Top-level directory redirection
- Unprivileged users can benefit from directory-level deduplication

**Symlinks**:
- Individual model file links: `model.safetensors` → `cas/blake3/ab/abc...`
- Cross-volume file deduplication (when privileges available)
- Preferred method for file-level operations

**modeld Strategy**:
1. Use hardlinks for same-volume files (always works, no privilege)
2. Use symlinks for cross-volume files (requires privilege)
3. Use junctions for virtual FS directories (no privilege needed)
4. Fall back to reference-only when none of the above work

#### 4. Filesystem Type Differences

**NTFS (New Technology File System)**:
- **Support**: Hardlinks ✓, Symlinks ✓ (with privilege), Junctions ✓
- **Prevalence**: Default Windows filesystem (C:\\ drive)
- **Characteristics**: Case-insensitive (default), NTFS Security, Compression, Encryption
- **modeld Status**: Fully supported

**ReFS (Resilient File System)**:
- **Support**: Hardlinks ✓, Symlinks ✓ (with privilege), Junctions ✓
- **Prevalence**: Windows Server, Storage Spaces, rarely on client systems
- **Characteristics**: Data integrity checksums, auto-repair, no compression/encryption
- **modeld Status**: Fully supported (same as NTFS)

**exFAT (Extended File Allocation Table)**:
- **Support**: Hardlinks ✗, Symlinks ✗, Junctions ✗
- **Prevalence**: USB drives, SD cards, external storage
- **Characteristics**: Large file support (>4GB), cross-platform (Windows, macOS, Linux)
- **modeld Status**: **Reference-only mode** (no linking capabilities)

**FAT32 (File Allocation Table 32)**:
- **Support**: Hardlinks ✗, Symlinks ✗, Junctions ✗
- **Prevalence**: Old USB drives, legacy systems
- **Characteristics**: 4GB file limit, no permissions, no advanced features
- **modeld Status**: **Not recommended** (4GB limit blocks most models), reference-only if needed

**Detection**:
```rust
fn get_filesystem_type(path: &Path) -> std::io::Result<String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use winapi::um::fileapi::GetVolumeInformationW;
        
        let volume_path = get_volume_path(path)?;
        let wide_volume: Vec<u16> = volume_path.as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        
        let mut fs_name_buf = vec![0u16; 32];
        
        unsafe {
            let result = GetVolumeInformationW(
                wide_volume.as_ptr(),
                std::ptr::null_mut(), 0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                fs_name_buf.as_mut_ptr(),
                fs_name_buf.len() as u32
            );
            
            if result != 0 {
                let len = fs_name_buf.iter().position(|&c| c == 0).unwrap_or(0);
                return Ok(String::from_utf16_lossy(&fs_name_buf[..len]));
            }
        }
    }
    
    Err(std::io::Error::last_os_error())
}
```

**Filesystem Capability Matrix**:

| Filesystem | Hardlinks | Symlinks (with priv) | Junctions | Recommended for modeld |
|------------|-----------|---------------------|-----------|----------------------|
| **NTFS** | ✓ | ✓ | ✓ | **Excellent** |
| **ReFS** | ✓ | ✓ | ✓ | **Excellent** |
| **exFAT** | ✗ | ✗ | ✗ | Reference-only |
| **FAT32** | ✗ | ✗ | ✗ | **Not Supported** |

#### 5. Path Length Limitations

**Legacy MAX_PATH (260 characters)**:
- Default Windows limit for file paths
- Many applications still affected
- Includes drive letter, path separators, filename, null terminator

**Long Path Support (Windows 10 1607+)**:
- Enable via registry: `HKLM\SYSTEM\CurrentControlSet\Control\FileSystem\LongPathsEnabled=1`
- Or via Group Policy: Computer Configuration → Administrative Templates → System → Filesystem
- Requires application manifest opt-in: `<longPathAware>true</longPathAware>`

**modeld Approach**:
```rust
// Use \\?\ prefix for long path support
fn normalize_path_windows(path: &Path) -> PathBuf {
    let path_str = path.to_string_lossy();
    
    if path_str.len() > 260 && !path_str.starts_with("\\\\?\\") {
        // Convert to absolute path and add \\?\ prefix
        let absolute = path.canonicalize().unwrap_or(path.to_path_buf());
        let absolute_str = absolute.to_string_lossy();
        
        // Add long path prefix
        if absolute_str.starts_with("\\\\") {
            // UNC path: \\server\share → \\?\UNC\server\share
            PathBuf::from(format!("\\\\?\\UNC\\{}", &absolute_str[2..]))
        } else {
            // Regular path: C:\path → \\?\C:\path
            PathBuf::from(format!("\\\\?\\{}", absolute_str))
        }
    } else {
        path.to_path_buf()
    }
}
```

**Impact**: Low - CAS hash-based paths are long but predictable, rarely exceeding 260 chars.

**Example**:
```
C:\modeld\cas\blake3\ab\abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890
^-- 89 characters (well under 260 limit)
```

## Proposed Design: Multi-Tier Link Strategy

### Link Strategy Decision Tree

```
┌─────────────────────────────────────────────────────────────────┐
│ START: Need to create link from source → CAS target             │
└────────────────────────────┬────────────────────────────────────┘
                             │
                             ▼
                    ┌────────────────────┐
                    │ Check filesystem   │
                    │ type of source     │
                    └────────┬───────────┘
                             │
              ┌──────────────┼──────────────┐
              │              │              │
          exFAT/FAT32      NTFS/ReFS    Unknown
              │              │              │
              ▼              ▼              │
    Reference-Only         │              │
        (goto END)         │              │
                           │              │
                           ▼              │
                  ┌─────────────────┐     │
                  │ Check: Same     │     │
                  │ volume?         │     │
                  └────────┬────────┘     │
                           │              │
                  ┌────────┴────────┐     │
                  │                 │     │
                 YES               NO     │
                  │                 │     │
                  ▼                 ▼     ▼
          ┌──────────────┐   ┌──────────────────┐
          │ Create       │   │ Check: Have      │
          │ HARDLINK     │   │ symlink          │
          │              │   │ privilege?       │
          └──────┬───────┘   └────────┬─────────┘
                 │                    │
                 │           ┌────────┴────────┐
                 │           │                 │
                 │          YES               NO
                 │           │                 │
                 │           ▼                 ▼
                 │   ┌──────────────┐   ┌──────────────┐
                 │   │ Create       │   │ Reference-   │
                 │   │ SYMLINK      │   │ Only Mode    │
                 │   │              │   │              │
                 │   └──────┬───────┘   └──────┬───────┘
                 │          │                  │
                 │          │                  │
                 └──────────┴──────────────────┘
                            │
                            ▼
                    ┌────────────────┐
                    │ Verify link    │
                    │ created        │
                    └───────┬────────┘
                            │
                   ┌────────┴────────┐
                   │                 │
                Success           Failure
                   │                 │
                   ▼                 ▼
            ┌──────────┐      ┌──────────────┐
            │ Update   │      │ Log error,   │
            │ aliases  │      │ fallback to  │
            │ table    │      │ reference    │
            └─────┬────┘      └──────┬───────┘
                  │                  │
                  └──────────────────┘
                           │
                           ▼
                        ┌──────┐
                        │ END  │
                        └──────┘
```

### Pseudocode Implementation

```rust
enum LinkType {
    Hardlink,
    Symlink,
    ReferenceOnly,
}

enum LinkResult {
    Success(LinkType),
    Failed(String),  // Error message
}

struct LinkCapability {
    has_symlink_privilege: bool,
    filesystem_type: String,
}

// Main entry point for creating a link
fn create_link(source: &Path, target: &Path, capability: &LinkCapability) -> LinkResult {
    // Step 1: Check filesystem type
    let fs_type = get_filesystem_type(source)
        .unwrap_or_else(|_| "UNKNOWN".to_string());
    
    if fs_type == "FAT32" || fs_type == "exFAT" {
        // No linking support on FAT32/exFAT
        log::warn!("Filesystem {} does not support links: {}", fs_type, source.display());
        return create_reference_only(source, target);
    }
    
    // Step 2: Check if same volume
    if is_same_volume(source, target) {
        // Same volume - try hardlink
        log::debug!("Same volume detected, attempting hardlink");
        return create_hardlink(source, target);
    }
    
    // Step 3: Cross-volume - check privilege
    if capability.has_symlink_privilege {
        // Have privilege - try symlink
        log::debug!("Cross-volume with privilege, attempting symlink");
        return create_symlink(source, target);
    }
    
    // Step 4: No privilege - reference-only mode
    log::warn!("Cross-volume without privilege: {}", source.display());
    return create_reference_only(source, target);
}

fn create_hardlink(source: &Path, target: &Path) -> LinkResult {
    match std::fs::hard_link(target, source) {
        Ok(_) => {
            log::info!("Created hardlink: {} → {}", source.display(), target.display());
            LinkResult::Success(LinkType::Hardlink)
        },
        Err(e) => {
            log::error!("Hardlink failed: {}", e);
            LinkResult::Failed(format!("Hardlink error: {}", e))
        }
    }
}

fn create_symlink(source: &Path, target: &Path) -> LinkResult {
    #[cfg(windows)]
    {
        use std::os::windows::fs::symlink_file;
        
        match symlink_file(target, source) {
            Ok(_) => {
                log::info!("Created symlink: {} → {}", source.display(), target.display());
                LinkResult::Success(LinkType::Symlink)
            },
            Err(e) => {
                log::error!("Symlink failed: {}", e);
                
                // Check error code
                if e.raw_os_error() == Some(1314) {  // ERROR_PRIVILEGE_NOT_HELD
                    log::warn!("Privilege check was incorrect, falling back to reference-only");
                }
                
                LinkResult::Failed(format!("Symlink error: {}", e))
            }
        }
    }
    
    #[cfg(not(windows))]
    {
        LinkResult::Failed("Symlink only supported on Windows in this context".to_string())
    }
}

fn create_reference_only(source: &Path, target: &Path) -> LinkResult {
    // File remains at original location
    // Record in aliases table with type='reference_only'
    log::info!("Reference-only mode: file stays at {}", source.display());
    
    LinkResult::Success(LinkType::ReferenceOnly)
}
```

### Privilege Detection Strategy

**Detection Timing**:
1. **On daemon startup**: Test once, cache result
2. **On first dedup operation**: Lazy detection
3. **User can override**: `modeld config set symlink_privilege true/false`

**Cache Result**:
```rust
use once_cell::sync::OnceCell;

static LINK_CAPABILITY: OnceCell<LinkCapability> = OnceCell::new();

fn get_link_capability() -> &'static LinkCapability {
    LINK_CAPABILITY.get_or_init(|| {
        let has_privilege = detect_symlink_privilege();
        let fs_type = detect_primary_filesystem();
        
        log::info!("Link capability detected: symlink_privilege={}, primary_fs={}", 
                   has_privilege, fs_type);
        
        LinkCapability {
            has_symlink_privilege: has_privilege,
            filesystem_type: fs_type,
        }
    })
}

fn detect_symlink_privilege() -> bool {
    let temp_dir = std::env::temp_dir();
    let test_target = temp_dir.join("modeld_test_target.txt");
    let test_link = temp_dir.join("modeld_test_link.txt");
    
    // Create target
    if std::fs::write(&test_target, "test").is_err() {
        return false;
    }
    
    // Try symlink
    let result = std::os::windows::fs::symlink_file(&test_target, &test_link);
    
    // Cleanup
    let _ = std::fs::remove_file(&test_link);
    let _ = std::fs::remove_file(&test_target);
    
    result.is_ok()
}
```

### Fallback Strategy Matrix

| Scenario | Source FS | Target FS | Same Volume | Has Privilege | Strategy | Space Saved |
|----------|-----------|-----------|-------------|---------------|----------|-------------|
| **A** | NTFS | NTFS | ✓ Yes | N/A | **Hardlink** | 100% |
| **B** | NTFS | NTFS | ✗ No | ✓ Yes | **Symlink** | 100% |
| **C** | NTFS | NTFS | ✗ No | ✗ No | **Reference-Only** | 0% |
| **D** | NTFS | ReFS | ✗ No | ✓ Yes | **Symlink** | 100% |
| **E** | exFAT | NTFS | ✗ No | Any | **Reference-Only** | 0% |
| **F** | NTFS | exFAT | ✗ No | Any | **Reference-Only** | 0% |
| **G** | FAT32 | NTFS | ✗ No | Any | **Not Supported** | N/A |

**Key Insights**:
- **Scenario A** (same volume): Always works, best case (zero overhead hardlink)
- **Scenario B** (cross volume, privileged): Full functionality, common for Developer Mode users
- **Scenario C** (cross volume, unprivileged): Degraded mode, **affects 90% of users initially**
- **Scenario E/F** (exFAT involved): No space savings possible, reference tracking only

**User Impact Distribution** (estimated):

| Configuration | % of Users | modeld Functionality |
|---------------|-----------|---------------------|
| Single NTFS volume + no privilege | ~30% | Full dedup (hardlinks) |
| Multi-volume NTFS + Developer Mode | ~5-8% | Full dedup (hardlinks + symlinks) |
| Multi-volume NTFS + no privilege | **~60%** | **Limited dedup** (hardlinks same-volume only) |
| exFAT/FAT32 users | ~2-5% | Reference-only mode |

**Critical Observation**: ~60% of users will have limited functionality without Developer Mode guidance.

## User Communication Strategy

### Setup Wizard / First-Run Experience

**Detection & Recommendation**:

```
$ modeld init

Initializing modeld...

Checking system capabilities:
  ✓ Windows 10/11 detected
  ✓ NTFS filesystem on C:\
  ⚠ Symlink privilege: NOT AVAILABLE

╔════════════════════════════════════════════════════════════════╗
║                      Limited Functionality                      ║
╚════════════════════════════════════════════════════════════════╝

modeld can save disk space by deduplicating identical models. However,
your system configuration limits deduplication capabilities:

  Current Mode: Limited (same-volume only)
  - Models on C:\ can be fully deduplicated ✓
  - Models on other drives (D:\, E:\, etc.) cannot be deduplicated ✗

To enable full deduplication across all drives:

  1. Open Windows Settings
  2. Go to: Update & Security → For Developers
  3. Enable: Developer Mode
  4. Restart modeld

This is a one-time setup and does not require Administrator privileges.

Learn more: https://modeld.dev/docs/windows-setup

Continue with limited mode? [Y/n]:
```

### Warning Messages

**During Scan (Cross-Volume Detection)**:

```
$ modeld scan D:\ComfyUI\models E:\Forge\models

Scanning directories...
[████████████████████] 100% (150 files scanned)

Found 25 duplicate groups (78.4 GB potential savings)

╔════════════════════════════════════════════════════════════════╗
║                    Deduplication Limitations                    ║
╚════════════════════════════════════════════════════════════════╝

Cross-volume duplicates detected:
  • 15 groups spanning C:\ ↔ D:\ (45.2 GB)
  • 10 groups spanning D:\ ↔ E:\ (33.2 GB)

Without symlink privileges, these cannot be deduplicated.

Actions:
  1. Enable Developer Mode (recommended)
     → Full deduplication: 78.4 GB savings
  
  2. Move all models to single drive
     → Dedup within drive: ~35 GB savings
  
  3. Continue with current setup
     → Reference tracking only (no space savings)

Setup guide: modeld docs windows-setup
```

### Dedup Operation Warnings

**Mixed Results**:

```
$ modeld dedup

Deduplicating 25 groups...
[████████████████████] 100% (25/25 groups processed)

Results:
  ✓ Successfully deduplicated: 10 groups (35.2 GB saved)
  ⚠ Limited by permissions: 15 groups (43.2 GB not saved)

Breakdown:
  • Same volume (C:\ only): 10 groups → 35.2 GB freed ✓
  • Cross volume (no privilege): 15 groups → Reference-only mode ⚠

To unlock full savings:
  1. Enable Developer Mode in Windows Settings
  2. Run: modeld dedup --retry-failed

Learn more: modeld docs windows-setup
```

### Status Command Output

```
$ modeld status

modeld Status:
  Version: 0.1.0
  Store: C:\modeld\
  Database: 1,245 models indexed

System Capabilities:
  OS: Windows 11 (22H2)
  Filesystem: NTFS (C:\), NTFS (D:\), exFAT (E:\)
  Symlink Privilege: ✗ Not Available
  
Link Strategy:
  • C:\ ↔ C:\: Hardlink (full dedup) ✓
  • C:\ ↔ D:\: Reference-only (no priv) ⚠
  • C:\ ↔ E:\: Reference-only (exFAT) ⚠
  
Space Savings:
  Potential: 125.6 GB (45 duplicate groups)
  Actual: 42.3 GB (15 groups, same-volume only)
  Locked: 83.3 GB (30 groups, need Developer Mode)

Enable full dedup: modeld docs windows-setup
```

### Documentation Requirements

**Windows Setup Guide** (`docs/windows-setup.md`):

1. **Overview**: Why Developer Mode is needed
2. **Step-by-step instructions** with screenshots:
   - Open Settings app
   - Navigate to "Update & Security" → "For Developers"
   - Toggle Developer Mode ON
   - Accept UAC prompt
   - Wait for components to install
3. **Verification**: How to check if it worked (`modeld status`)
4. **Troubleshooting**: Common issues
   - Group Policy blocking Developer Mode (enterprise)
   - Windows Home vs Pro differences
   - Alternative: Running as Administrator (not recommended)
5. **FAQ**:
   - "Is Developer Mode safe?" → Yes for personal machines
   - "Will this affect other programs?" → No, only grants symlink permission
   - "Can I revert?" → Yes, toggle off anytime

**Comparison Table**:

| Feature | Without Developer Mode | With Developer Mode |
|---------|----------------------|-------------------|
| Same-volume dedup | ✓ Full | ✓ Full |
| Cross-volume dedup | ✗ Blocked | ✓ Full |
| Space savings | Partial (~30-50%) | Maximum (~100%) |
| Setup complexity | None | 5-minute one-time setup |
| Security impact | None | None (safe) |
| Administrator needed | No | No (just toggle) |

## Developer Mode Setup Process

### Detailed Instructions

**Method 1: Settings UI** (Recommended)

1. **Open Settings**:
   - Press `Win + I`
   - Or: Start Menu → Settings (gear icon)

2. **Navigate to Developer Settings**:
   - Windows 11: Settings → Privacy & Security → For developers
   - Windows 10: Settings → Update & Security → For developers

3. **Enable Developer Mode**:
   - Find "Developer Mode" toggle
   - Switch to ON position
   - Accept UAC prompt (if appears)
   - Wait for "Developer Mode package" to install (~1-2 minutes)

4. **Verify**:
   ```powershell
   # PowerShell command to verify
   whoami /priv | findstr "SeCreateSymbolicLinkPrivilege"
   ```
   Expected output: `SeCreateSymbolicLinkPrivilege` should appear

**Method 2: Registry** (Advanced Users)

```powershell
# PowerShell (Run as Administrator)
Set-ItemProperty -Path "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock" `
                 -Name "AllowDevelopmentWithoutDevLicense" -Value 1 -Type DWord
```

**Method 3: Group Policy** (Enterprise/Pro)

```
1. Run: gpedit.msc
2. Navigate to: Computer Configuration → Administrative Templates → 
   System → Device Installation
3. Find: "Enable Developer Mode"
4. Set to: Enabled
5. Apply and restart
```

### Troubleshooting

**Issue 1: Developer Mode Toggle Grayed Out**

**Symptoms**: Cannot toggle Developer Mode switch

**Causes**:
- Group Policy restriction (enterprise environment)
- Windows Home edition limitation (should work, but sometimes buggy)
- Windows Update required

**Solutions**:
```powershell
# Check Group Policy restriction
Get-ItemProperty -Path "HKLM:\SOFTWARE\Policies\Microsoft\Windows\Appx" `
                 -Name "AllowDevelopmentWithoutDevLicense" -ErrorAction SilentlyContinue

# If blocked by policy, contact IT admin or use alternative method
```

**Issue 2: "Developer Mode package couldn't be installed"**

**Symptoms**: Error during installation

**Solutions**:
1. Check Windows Update (install pending updates)
2. Run Windows Update Troubleshooter
3. Manually install: Settings → Apps → Optional Features → Add "Windows Developer Mode"

**Issue 3: Privilege Still Not Working After Enabling**

**Symptoms**: modeld still reports no privilege

**Solutions**:
```powershell
# 1. Restart modeld daemon
modeld daemon restart

# 2. Verify privilege manually
whoami /priv | findstr "SeCreateSymbolicLinkPrivilege"

# 3. Re-test in modeld
modeld status --refresh-capabilities

# 4. Last resort: Log out and log back in (or reboot)
```

**Issue 4: Enterprise Policy Blocks Developer Mode**

**Symptoms**: IT policy prevents enabling

**Solutions**:
- **Option A**: Request exception from IT department (explain use case)
- **Option B**: Use single-volume setup (move all models to one drive)
- **Option C**: Accept limited mode (same-volume dedup only)

## Testing Matrix for Windows Scenarios

### Test Scenarios

| Test ID | Scenario | Setup | Expected Behavior | Validation |
|---------|----------|-------|-------------------|------------|
| **W-01** | Same volume, NTFS, no privilege | C:\ → C:\, NTFS, Dev Mode OFF | Hardlink created | `fsutil hardlink list` |
| **W-02** | Cross volume, NTFS, with privilege | C:\ → D:\, NTFS, Dev Mode ON | Symlink created | `dir /AL` shows symlink |
| **W-03** | Cross volume, NTFS, no privilege | C:\ → D:\, NTFS, Dev Mode OFF | Reference-only, warning | Check aliases table |
| **W-04** | Same volume, ReFS | C:\ → C:\, ReFS | Hardlink created | `fsutil hardlink list` |
| **W-05** | Cross volume, ReFS, with privilege | C:\ → D:\, ReFS, Dev Mode ON | Symlink created | `dir /AL` |
| **W-06** | exFAT source | E:\ (exFAT) → C:\ (NTFS) | Reference-only, warning | Check error log |
| **W-07** | exFAT target | C:\ (NTFS) → E:\ (exFAT) | Reference-only, warning | Check error log |
| **W-08** | FAT32 source | F:\ (FAT32) → C:\ | Not supported error | Error message |
| **W-09** | Privilege detection | N/A | Correct capability reported | `modeld status` |
| **W-10** | Volume detection | Multiple drives | Correct volume paths | Log validation |
| **W-11** | Filesystem type detection | NTFS, ReFS, exFAT, FAT32 | Correct types reported | `modeld status --verbose` |
| **W-12** | Long path support | >260 char path | Path normalized with \\\\?\\ | Check file operations |
| **W-13** | Junction for virtual FS | Directory link | Junction created (no priv) | `dir /AL` |
| **W-14** | Dedup retry after enabling Dev Mode | Enable Dev Mode mid-session | Previously failed links succeed | `modeld dedup --retry` |
| **W-15** | USB drive (exFAT) models | External drive | Reference-only gracefully | No crashes |

### Automated Test Suite

```rust
#[cfg(test)]
mod windows_compat_tests {
    use super::*;

    #[test]
    #[cfg(target_os = "windows")]
    fn test_privilege_detection() {
        let capability = get_link_capability();
        
        // Should not crash
        assert!(capability.has_symlink_privilege == true || 
                capability.has_symlink_privilege == false);
        
        println!("Detected privilege: {}", capability.has_symlink_privilege);
    }
    
    #[test]
    #[cfg(target_os = "windows")]
    fn test_volume_detection() {
        let c_drive = Path::new("C:\\");
        let d_drive = Path::new("D:\\");
        
        let c_volume = get_volume_path(c_drive).unwrap();
        assert_eq!(c_volume.to_str().unwrap().to_uppercase(), "C:\\");
        
        // D: may not exist, handle gracefully
        if d_drive.exists() {
            let d_volume = get_volume_path(d_drive).unwrap();
            assert_eq!(d_volume.to_str().unwrap().to_uppercase(), "D:\\");
            assert!(!is_same_volume(c_drive, d_drive));
        }
    }
    
    #[test]
    #[cfg(target_os = "windows")]
    fn test_filesystem_detection() {
        let c_drive = Path::new("C:\\");
        let fs_type = get_filesystem_type(c_drive).unwrap();
        
        // C: is usually NTFS or ReFS
        assert!(fs_type == "NTFS" || fs_type == "ReFS", 
                "Unexpected filesystem: {}", fs_type);
    }
    
    #[test]
    #[cfg(target_os = "windows")]
    fn test_hardlink_same_volume() {
        use tempfile::tempdir;
        
        let temp = tempdir().unwrap();
        let target = temp.path().join("target.txt");
        let link = temp.path().join("link.txt");
        
        std::fs::write(&target, "test content").unwrap();
        
        let result = create_hardlink(&link, &target);
        assert!(matches!(result, LinkResult::Success(LinkType::Hardlink)));
        
        // Verify content
        let link_content = std::fs::read_to_string(&link).unwrap();
        assert_eq!(link_content, "test content");
    }
    
    #[test]
    #[cfg(target_os = "windows")]
    fn test_symlink_with_privilege() {
        use tempfile::tempdir;
        
        let capability = get_link_capability();
        if !capability.has_symlink_privilege {
            println!("Skipping: No symlink privilege");
            return;
        }
        
        let temp = tempdir().unwrap();
        let target = temp.path().join("target.txt");
        let link = temp.path().join("link.txt");
        
        std::fs::write(&target, "test content").unwrap();
        
        let result = create_symlink(&link, &target);
        assert!(matches!(result, LinkResult::Success(LinkType::Symlink)));
        
        // Verify it's a symlink
        let metadata = std::fs::symlink_metadata(&link).unwrap();
        assert!(metadata.file_type().is_symlink());
    }
    
    #[test]
    #[cfg(target_os = "windows")]
    fn test_long_path_normalization() {
        let short_path = Path::new("C:\\test\\file.txt");
        let normalized = normalize_path_windows(short_path);
        assert_eq!(normalized, short_path);
        
        // Create a very long path (>260 chars)
        let long_component = "a".repeat(100);
        let long_path_str = format!("C:\\{}\\{}\\{}\\file.txt", 
                                    long_component, long_component, long_component);
        let long_path = Path::new(&long_path_str);
        
        let normalized = normalize_path_windows(long_path);
        assert!(normalized.to_str().unwrap().starts_with("\\\\?\\"));
    }
}
```

### Manual Test Checklist

**Pre-Testing Setup**:
- [ ] Windows 10 (21H2+) or Windows 11 test machine
- [ ] Multiple drives: C:\\ (NTFS), D:\\ (NTFS), E:\\ (exFAT USB)
- [ ] Test with Developer Mode OFF initially
- [ ] Test models: 1-2 GB files (duplicated across drives)

**Test Execution**:

1. **Initial State (No Privilege)**:
   - [ ] Run `modeld scan C:\models D:\models`
   - [ ] Verify warning message appears (cross-volume limitation)
   - [ ] Run `modeld dedup`
   - [ ] Verify same-volume models deduplicated (C:\\ → C:\\)
   - [ ] Verify cross-volume models in reference-only mode
   - [ ] Check `modeld status` shows correct capability

2. **Enable Developer Mode**:
   - [ ] Follow setup instructions
   - [ ] Enable Developer Mode in Settings
   - [ ] Restart modeld daemon: `modeld daemon restart`
   - [ ] Verify `modeld status` now shows privilege available

3. **Retry Deduplication**:
   - [ ] Run `modeld dedup --retry-failed`
   - [ ] Verify cross-volume links now created (symlinks)
   - [ ] Check file properties: `dir /AL D:\models\`
   - [ ] Verify space savings reported correctly

4. **exFAT USB Drive**:
   - [ ] Plug in USB drive formatted as exFAT
   - [ ] Copy models to E:\\models
   - [ ] Run `modeld scan E:\models`
   - [ ] Verify warning about exFAT limitations
   - [ ] Confirm reference-only mode (no crashes)

5. **Junction Test (Virtual FS)**:
   - [ ] Run `modeld link comfyui --model-dir D:\ComfyUI\models`
   - [ ] Verify junction created (even without privilege)
   - [ ] Check with: `dir /AL D:\ComfyUI\models`
   - [ ] Should show junction point to modeld virtual FS

6. **Long Path Test**:
   - [ ] Create deep directory structure (>260 chars)
   - [ ] Place model file at end
   - [ ] Run `modeld scan` on that directory
   - [ ] Verify no path length errors

7. **Privilege Loss Simulation**:
   - [ ] Disable Developer Mode
   - [ ] Restart modeld: `modeld daemon restart`
   - [ ] Run `modeld status`
   - [ ] Verify privilege now reported as unavailable
   - [ ] Attempt new dedup → should fall back to reference-only

**Expected Results Summary**:

| Test Phase | Expected Behavior | Pass/Fail |
|------------|------------------|-----------|
| No privilege, same volume | Hardlinks created | ☐ |
| No privilege, cross volume | Reference-only + warning | ☐ |
| With privilege, cross volume | Symlinks created | ☐ |
| exFAT filesystem | Reference-only, no errors | ☐ |
| Junction creation | Works without privilege | ☐ |
| Long paths (>260 chars) | Handled correctly | ☐ |
| Privilege detection | Accurate reporting | ☐ |

## Edge Cases and Error Handling

### Edge Case 1: Mid-Operation Privilege Change

**Scenario**: User enables Developer Mode while modeld daemon is running

**Handling**:
```rust
// Periodic capability refresh (every 5 minutes or on command)
fn refresh_capabilities() {
    let new_capability = detect_capabilities();
    
    if new_capability.has_symlink_privilege != LINK_CAPABILITY.get().unwrap().has_symlink_privilege {
        log::info!("Privilege status changed! Updating capability cache...");
        
        // Force re-detection
        LINK_CAPABILITY.take();  // Clear old value
        let _ = get_link_capability();  // Re-detect
        
        // Notify user
        println!("✓ Symlink privilege now available! Run 'modeld dedup --retry-failed' to retry cross-volume links.");
    }
}
```

**User Command**:
```bash
modeld refresh-capabilities
modeld dedup --retry-failed
```

### Edge Case 2: Network Share (UNC Paths)

**Scenario**: User has models on network share `\\\\server\\models`

**Handling**:
```rust
fn is_network_path(path: &Path) -> bool {
    path.to_str()
        .map(|s| s.starts_with("\\\\") || s.starts_with("//"))
        .unwrap_or(false)
}

// In link creation logic
if is_network_path(source) || is_network_path(target) {
    log::warn!("Network paths detected: {}", source.display());
    log::warn!("Deduplication on network shares is not recommended (performance, reliability)");
    
    // Offer reference-only mode
    return create_reference_only(source, target);
}
```

### Edge Case 3: Filesystem Type Changes

**Scenario**: User reformats D:\\ from NTFS to exFAT

**Handling**:
- Re-detect filesystem type on each operation (cache per session)
- If existing symlinks become invalid, mark as broken in database
- Provide repair command: `modeld repair --check-links`

### Edge Case 4: Permission Denied (File Locks)

**Scenario**: File in use by another process (ComfyUI loading model)

**Handling**:
```rust
fn create_link_with_retry(source: &Path, target: &Path, max_retries: u32) -> LinkResult {
    for attempt in 1..=max_retries {
        match create_link(source, target, get_link_capability()) {
            LinkResult::Success(link_type) => return LinkResult::Success(link_type),
            LinkResult::Failed(err) if err.contains("Permission denied") || err.contains("Access denied") => {
                if attempt < max_retries {
                    log::debug!("File locked, retry {}/{}", attempt, max_retries);
                    std::thread::sleep(std::time::Duration::from_millis(500 * attempt as u64));
                } else {
                    log::error!("File locked after {} retries: {}", max_retries, source.display());
                    return LinkResult::Failed(err);
                }
            },
            LinkResult::Failed(err) => return LinkResult::Failed(err),
        }
    }
    
    LinkResult::Failed("Max retries exceeded".to_string())
}
```

### Edge Case 5: Partial Dedup Failure

**Scenario**: Dedup operation fails mid-way (crash, disk full, etc.)

**Handling**:
- Use WAL (Write-Ahead Logging) from RFC 0004
- On restart, detect incomplete transactions
- Rollback or resume based on transaction state
- Provide manual recovery: `modeld recover --verify-links`

## Design Alternatives Considered

### Alternative 1: Require Administrator Privileges Always

**Approach**: Force users to run modeld as Administrator

**Pros**:
- Symlinks always available
- Simpler code (no fallback logic)

**Cons**:
- **Terrible UX** (UAC prompt every time)
- **Security concern** (unnecessary elevation)
- **User resistance** (many avoid admin tools)

**Decision**: **Rejected** - User experience is critical, unnecessary privilege elevation is bad security practice.

---

### Alternative 2: Copy Files Instead of Links

**Approach**: When links fail, copy files to CAS and keep originals

**Pros**:
- Always "works" (no permission issues)
- Full CAS coverage

**Cons**:
- **Defeats purpose** (no space savings!)
- **Doubles disk usage** (original + CAS copy)
- **Confusing** (users expect deduplication to save space)

**Decision**: **Rejected** - Reference-only mode is more honest about limitations.

---

### Alternative 3: Windows-Only Product (No Cross-Platform)

**Approach**: Focus exclusively on Windows, use Windows-specific features

**Pros**:
- Could use Windows Storage Spaces dedup
- Native Windows APIs throughout

**Cons**:
- **Excludes Linux/macOS users** (large AI community)
- **Limits market** (many developers use Linux)
- **Fragmentation** (users with mixed systems)

**Decision**: **Rejected** - Cross-platform support is a core value.

---

### Alternative 4: FUSE Filesystem for Windows

**Approach**: Use WinFsp (Windows FUSE) to create virtual filesystem

**Pros**:
- Transparent file access
- No linking required

**Cons**:
- **Complex dependency** (requires driver installation)
- **Performance overhead** (FUSE layer)
- **Stability concerns** (third-party driver)
- **User resistance** (driver installation scary)

**Decision**: **Rejected** - Too complex, heavyweight solution for problem already solvable with links.

---

### Alternative 5: Ignore Windows, Document as "Limited Support"

**Approach**: Focus on Linux/macOS, Windows is "best effort"

**Pros**:
- Simpler development
- Less testing required

**Cons**:
- **Alienates majority** (60-70% of AI users on Windows)
- **Bad reputation** (incomplete product)
- **Missed opportunity** (Windows issues are solvable)

**Decision**: **Rejected** - Windows is too important to ignore.

---

### Alternative 6: Cloud-Based Dedup Service

**Approach**: Upload hashes to cloud service, download shared CAS objects

**Pros**:
- Works regardless of local filesystem
- Global deduplication (across all users)

**Cons**:
- **Privacy concerns** (model hashes reveal usage)
- **Internet dependency** (offline breaks)
- **Bandwidth cost** (downloading CAS objects)
- **Complexity** (authentication, storage, billing)

**Decision**: **Rejected** - Local-first approach is core principle, cloud can be future enhancement.

## Implementation Roadmap

### Phase 1: Core Detection (Week 1)

- [ ] Implement `has_symlink_privilege()` detection
- [ ] Implement `get_volume_path()` and `is_same_volume()`
- [ ] Implement `get_filesystem_type()`
- [ ] Cache capability detection results
- [ ] Unit tests for all detection functions

### Phase 2: Link Strategy (Week 2)

- [ ] Implement link strategy decision tree
- [ ] `create_hardlink()` with error handling
- [ ] `create_symlink()` with privilege check
- [ ] `create_reference_only()` database recording
- [ ] Integration tests for all link types

### Phase 3: User Communication (Week 3)

- [ ] Warning messages for privilege limitations
- [ ] Status command capability reporting
- [ ] Dedup operation result breakdown
- [ ] Write Windows setup guide documentation
- [ ] Screenshots for Developer Mode setup

### Phase 4: Testing (Week 4)

- [ ] Automated test suite on Windows CI
- [ ] Manual testing on Windows 10/11
- [ ] Test all filesystem types (NTFS, ReFS, exFAT)
- [ ] Test privilege transitions (enable/disable Dev Mode)
- [ ] Long-running stability tests

### Phase 5: Edge Cases & Polish (Week 5)

- [ ] Handle network paths (UNC)
- [ ] Handle long paths (>260 chars)
- [ ] Retry logic for file locks
- [ ] Recovery command for broken links
- [ ] Performance profiling on Windows

## Security Considerations

### Privilege Escalation Risk

**Concern**: Could privilege detection or link creation be exploited?

**Mitigations**:
- Detection is read-only (no system modifications)
- Link creation uses standard Windows APIs (no custom drivers)
- No UAC bypass attempts
- All operations logged for audit

**Risk Level**: Low - Standard filesystem operations, no elevation attempts.

### Symlink Attack Surface

**Concern**: Malicious symlinks could redirect modeld to overwrite system files

**Mitigations**:
- modeld only creates links, never follows user-provided symlinks to write
- CAS paths are always computed (never user-controlled)
- Write operations always verify target is within $MODELD_STORE

**Risk Level**: Low - Write paths are validated, no arbitrary symlink following.

### Developer Mode Security Impact

**Concern**: Does enabling Developer Mode weaken Windows security?

**Analysis**:
- Developer Mode grants SeCreateSymbolicLinkPrivilege (symlink creation)
- Does NOT grant Administrator privileges
- Does NOT disable UAC or other security features
- Primary risk: Malicious software could create symlinks (but requires execution first)
- For personal machines (not corporate), risk is acceptable

**Recommendation**: Safe for personal use, corporate environments should evaluate policy.

**Risk Level**: Low for personal users, consult IT for enterprise.

## Performance Considerations

### Privilege Detection Overhead

**Cost**: One-time 10-20ms test on startup

**Mitigation**: Cache result in static variable, persist in config file

**Impact**: Negligible (amortized over session)

### Volume Detection Overhead

**Cost**: ~1-2ms per path (Windows API call)

**Mitigation**: Cache volume paths per session

**Impact**: Minimal (one-time per unique path)

### Symlink vs Hardlink Performance

**Read Performance**:
- Hardlink: Zero overhead (same inode)
- Symlink: ~1-5ms redirection overhead per open

**Space Overhead**:
- Hardlink: Zero (shared inode)
- Symlink: ~100 bytes (link metadata)

**Recommendation**: Prefer hardlinks when possible (same volume).

### Link Creation Latency

| Operation | Latency | Notes |
|-----------|---------|-------|
| Hardlink creation | ~0.5-1ms | Fast (metadata update) |
| Symlink creation | ~1-2ms | Slightly slower (reparse point) |
| Reference-only | ~0.1ms | Database update only |

**Impact**: For 1000 models, worst case ~2 seconds (all symlinks).

## Open Questions

1. **ReFS Dedup Integration**: Should we detect and integrate with ReFS built-in dedup?
   - **Action**: Research ReFS dedup APIs (likely Phase 2+)

2. **Storage Spaces Dedup**: Can we leverage Windows Storage Spaces dedup?
   - **Action**: Investigate if accessible via API

3. **OneDrive/Cloud Sync**: How to handle cloud-synced model directories?
   - **Action**: Detect cloud sync attributes, warn about potential issues

4. **WSL Integration**: Should modeld work from within WSL accessing Windows drives?
   - **Action**: Test WSL scenarios, document limitations

5. **Windows 11 Native Symlinks**: Does Win11 ease symlink restrictions?
   - **Action**: Test on Win11 latest builds, update docs if improved

## Summary

**Windows Compatibility Strategy**:

1. **Multi-Tier Fallback**:
   - Same volume → Hardlink (always works)
   - Cross volume + privilege → Symlink (requires Developer Mode)
   - Cross volume + no privilege → Reference-only (tracks duplicates, no space savings)
   - exFAT/FAT32 → Reference-only (no link support)

2. **Runtime Detection**:
   - Test symlink privilege on startup
   - Detect filesystem types per volume
   - Cache results for performance

3. **Clear User Communication**:
   - Explain limitations upfront
   - Provide step-by-step Developer Mode setup
   - Show potential vs actual savings
   - Offer retry after privilege grant

4. **Comprehensive Testing**:
   - Automated tests for all scenarios
   - Manual testing matrix covering edge cases
   - Continuous Windows CI testing

5. **Graceful Degradation**:
   - Never crash due to permission issues
   - Always provide partial functionality
   - Recover from privilege changes mid-session

**Success Criteria**:
- ✓ Works on fresh Windows 10/11 install (limited mode)
- ✓ Full functionality after simple Developer Mode setup
- ✓ Clear communication at every step
- ✓ No crashes on any filesystem type
- ✓ Recovers from privilege transitions

**User Impact**:
- ~30% of users: Full functionality immediately (single volume)
- ~60% of users: Full functionality after 5-minute setup (multi-volume + Dev Mode)
- ~10% of users: Limited functionality (enterprise policy, exFAT, etc.)

**Critical Success Factor**: Clear, non-technical documentation guiding users through Developer Mode setup.

---

## References

**Windows APIs**:
- `CreateSymbolicLinkW()`: [Microsoft Docs](https://docs.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createsymboliclinkw)
- `CreateHardLinkW()`: [Microsoft Docs](https://docs.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-createhardlinkw)
- `GetVolumePathNameW()`: [Microsoft Docs](https://docs.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getvolumepathnamew)
- `GetVolumeInformationW()`: [Microsoft Docs](https://docs.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-getvolumeinformationw)

**Developer Mode**:
- [Enable your device for development](https://docs.microsoft.com/en-us/windows/apps/get-started/enable-your-device-for-development)
- [Symlinks in Windows 10](https://blogs.windows.com/windowsdeveloper/2016/12/02/symlinks-windows-10/)

**Filesystem Documentation**:
- [NTFS Overview](https://docs.microsoft.com/en-us/windows-server/storage/file-server/ntfs-overview)
- [ReFS Overview](https://docs.microsoft.com/en-us/windows-server/storage/refs/refs-overview)
- [Hard Links and Junctions](https://docs.microsoft.com/en-us/windows/win32/fileio/hard-links-and-junctions)

---

*RFC 0005 Version: 1.0*  
*Status: Draft*  
*Priority: CRITICAL*  
*Last Updated: 2024*
