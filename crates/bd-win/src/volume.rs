//! Volume identity, capacity and capabilities (FR01, FR06, FR09).

use std::path::Path;

use bd_core::age::AccessReliability;
use windows::Win32::Storage::FileSystem::{
    FILE_ID_INFO, FileIdInfo, GetDiskFreeSpaceExW, GetDiskFreeSpaceW, GetDriveTypeW,
    GetFileInformationByHandleEx, GetVolumeInformationByHandleW, GetVolumeNameForVolumeMountPointW,
    GetVolumePathNameW,
};
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RegGetValueW};
use windows_core::{PCWSTR, w};

use crate::error;
use crate::handle::Handle;
use crate::wide;

const FILE_READ_ATTRIBUTES: u32 = 0x0080;

/// `GetVolumeInformation` file-system flags we report (winnt.h).
pub mod fs_flags {
    pub const CASE_SENSITIVE_SEARCH: u32 = 0x0000_0001;
    pub const CASE_PRESERVED_NAMES: u32 = 0x0000_0002;
    pub const UNICODE_ON_DISK: u32 = 0x0000_0004;
    pub const PERSISTENT_ACLS: u32 = 0x0000_0008;
    pub const FILE_COMPRESSION: u32 = 0x0000_0010;
    pub const SUPPORTS_SPARSE_FILES: u32 = 0x0000_0040;
    pub const SUPPORTS_REPARSE_POINTS: u32 = 0x0000_0080;
    pub const VOLUME_IS_COMPRESSED: u32 = 0x0000_8000;
    pub const SUPPORTS_ENCRYPTION: u32 = 0x0002_0000;
    pub const NAMED_STREAMS: u32 = 0x0004_0000;
    pub const READ_ONLY_VOLUME: u32 = 0x0008_0000;
    pub const SUPPORTS_HARD_LINKS: u32 = 0x0040_0000;
    pub const SUPPORTS_OPEN_BY_FILE_ID: u32 = 0x0100_0000;
    pub const SUPPORTS_USN_JOURNAL: u32 = 0x0200_0000;
    pub const SUPPORTS_BLOCK_REFCOUNTING: u32 = 0x0800_0000;

    pub const NAMES: [(u32, &str); 15] = [
        (CASE_SENSITIVE_SEARCH, "caseSensitiveSearch"),
        (CASE_PRESERVED_NAMES, "casePreservedNames"),
        (UNICODE_ON_DISK, "unicodeOnDisk"),
        (PERSISTENT_ACLS, "persistentAcls"),
        (FILE_COMPRESSION, "fileCompression"),
        (SUPPORTS_SPARSE_FILES, "sparseFiles"),
        (SUPPORTS_REPARSE_POINTS, "reparsePoints"),
        (VOLUME_IS_COMPRESSED, "volumeIsCompressed"),
        (SUPPORTS_ENCRYPTION, "encryption"),
        (NAMED_STREAMS, "namedStreams"),
        (READ_ONLY_VOLUME, "readOnlyVolume"),
        (SUPPORTS_HARD_LINKS, "hardLinks"),
        (SUPPORTS_OPEN_BY_FILE_ID, "openByFileId"),
        (SUPPORTS_USN_JOURNAL, "usnJournal"),
        (SUPPORTS_BLOCK_REFCOUNTING, "blockRefcounting"),
    ];

    pub fn describe(flags: u32) -> Vec<&'static str> {
        NAMES
            .iter()
            .filter(|(bit, _)| flags & bit != 0)
            .map(|&(_, name)| name)
            .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DriveType {
    Unknown,
    NoRootDir,
    Removable,
    Fixed,
    Remote,
    CdRom,
    RamDisk,
}

impl DriveType {
    fn from_raw(v: u32) -> DriveType {
        match v {
            1 => DriveType::NoRootDir,
            2 => DriveType::Removable,
            3 => DriveType::Fixed,
            4 => DriveType::Remote,
            5 => DriveType::CdRom,
            6 => DriveType::RamDisk,
            _ => DriveType::Unknown,
        }
    }
}

#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeInfo {
    /// Mount path containing the queried path, e.g. `C:\`.
    pub mount_path: String,
    /// Stable volume GUID path, e.g. `\\?\Volume{...}\`, when available.
    pub guid_path: Option<String>,
    pub label: String,
    pub file_system: String,
    /// 32-bit serial from `GetVolumeInformation`.
    pub serial32: u32,
    /// 64-bit serial from `FileIdInfo` (pairs with 128-bit file IDs).
    pub serial64: Option<u64>,
    pub max_component_length: u32,
    pub fs_flags: u32,
    pub fs_flag_names: Vec<&'static str>,
    pub drive_type: DriveType,
    pub total_bytes: u64,
    /// Free bytes on the volume.
    pub free_bytes: u64,
    /// Free bytes available to this user (quotas can make it smaller).
    pub available_bytes: u64,
    pub bytes_per_cluster: Option<u64>,
}

impl VolumeInfo {
    pub fn used_bytes(&self) -> u64 {
        self.total_bytes.saturating_sub(self.free_bytes)
    }

    pub fn is_ntfs(&self) -> bool {
        self.file_system.eq_ignore_ascii_case("NTFS")
    }
}

/// Query the volume that contains `path`. `path` must exist.
pub fn query(path: &Path) -> windows_core::Result<VolumeInfo> {
    let path_w = wide::to_wide_nul(path.as_os_str())?;

    let mut mount = vec![0u16; 1024];
    // SAFETY: valid NUL-terminated input and output buffer.
    unsafe { GetVolumePathNameW(PCWSTR(path_w.as_ptr()), &mut mount)? };
    let mount_os = wide::from_wide_until_nul(&mount);
    let mount_w = wide::to_wide_nul(&mount_os)?;
    let mount_pcwstr = PCWSTR(mount_w.as_ptr());

    let mut guid = vec![0u16; 64];
    // SAFETY: as above. Fails for paths that are not volume mount points
    // (e.g. network shares); that is reported as None.
    let guid_path = unsafe { GetVolumeNameForVolumeMountPointW(mount_pcwstr, &mut guid) }
        .ok()
        .map(|_| {
            wide::from_wide_until_nul(&guid)
                .to_string_lossy()
                .into_owned()
        });

    let handle = Handle::open(
        path,
        FILE_READ_ATTRIBUTES,
        windows::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS,
    )?;
    let mut label = vec![0u16; 261];
    let mut fs_name = vec![0u16; 261];
    let (mut serial32, mut max_component, mut flags) = (0u32, 0u32, 0u32);
    // SAFETY: all out-pointers are valid for the call.
    unsafe {
        GetVolumeInformationByHandleW(
            handle.raw(),
            Some(&mut label),
            Some(&mut serial32),
            Some(&mut max_component),
            Some(&mut flags),
            Some(&mut fs_name),
        )?;
    }

    let mut id_info = FILE_ID_INFO::default();
    // SAFETY: buffer is a FILE_ID_INFO of the size passed.
    let serial64 = unsafe {
        GetFileInformationByHandleEx(
            handle.raw(),
            FileIdInfo,
            (&raw mut id_info).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    }
    .ok()
    .map(|_| id_info.VolumeSerialNumber);

    // SAFETY: valid NUL-terminated input.
    let drive_type = DriveType::from_raw(unsafe { GetDriveTypeW(mount_pcwstr) });

    let (mut available, mut total, mut free) = (0u64, 0u64, 0u64);
    // SAFETY: out-pointers valid.
    unsafe {
        GetDiskFreeSpaceExW(
            mount_pcwstr,
            Some(&mut available),
            Some(&mut total),
            Some(&mut free),
        )?;
    }

    let (mut spc, mut bps, mut nfree, mut ntotal) = (0u32, 0u32, 0u32, 0u32);
    // SAFETY: out-pointers valid.
    let bytes_per_cluster = unsafe {
        GetDiskFreeSpaceW(
            mount_pcwstr,
            Some(&mut spc),
            Some(&mut bps),
            Some(&mut nfree),
            Some(&mut ntotal),
        )
    }
    .ok()
    .map(|_| u64::from(spc) * u64::from(bps));

    Ok(VolumeInfo {
        mount_path: mount_os.to_string_lossy().into_owned(),
        guid_path,
        label: wide::from_wide_until_nul(&label)
            .to_string_lossy()
            .into_owned(),
        file_system: wide::from_wide_until_nul(&fs_name)
            .to_string_lossy()
            .into_owned(),
        serial32,
        serial64,
        max_component_length: max_component,
        fs_flags: flags,
        fs_flag_names: fs_flags::describe(flags),
        drive_type,
        total_bytes: total,
        free_bytes: free,
        available_bytes: available,
        bytes_per_cluster,
    })
}

/// The NTFS last-access update policy (FR06: access dates are only
/// meaningful when the system updates them).
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccessTimePolicy {
    /// Raw `NtfsDisableLastAccessUpdate` value, when readable.
    pub raw: Option<u32>,
    /// Windows decides (bit 31 set). Since Windows 10 1803 the default is
    /// system-managed: at boot Windows enables updates only when the system
    /// volume is small, and records the decision in the low bit.
    pub system_managed: bool,
    /// Low bit set: updates are disabled.
    pub updates_disabled: Option<bool>,
    pub note: &'static str,
}

impl AccessTimePolicy {
    /// What the application may claim about access dates on a volume.
    pub fn reliability(&self, volume: &VolumeInfo) -> AccessReliability {
        if !volume.is_ntfs() {
            return AccessReliability::Unverified;
        }
        match self.updates_disabled {
            Some(true) => AccessReliability::Unavailable,
            Some(false) => AccessReliability::Known,
            None => AccessReliability::Unverified,
        }
    }
}

pub fn access_time_policy() -> AccessTimePolicy {
    let mut value = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: out-pointers valid; the value is a REG_DWORD.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("SYSTEM\\CurrentControlSet\\Control\\FileSystem"),
            w!("NtfsDisableLastAccessUpdate"),
            RRF_RT_REG_DWORD,
            None,
            Some((&raw mut value).cast()),
            Some(&mut size),
        )
    };
    if status.0 != 0 {
        return AccessTimePolicy {
            raw: None,
            system_managed: false,
            updates_disabled: None,
            note: "policy value unreadable; access dates are unverified",
        };
    }
    let system_managed = value & 0x8000_0000 != 0;
    let disabled = value & 1 != 0;
    AccessTimePolicy {
        raw: Some(value),
        system_managed,
        updates_disabled: Some(disabled),
        note: match (system_managed, disabled) {
            (true, true) => "system-managed, currently disabled",
            (true, false) => "system-managed, currently enabled",
            (false, true) => "user-managed, disabled",
            (false, false) => "user-managed, enabled (NTFS still delays updates by up to an hour)",
        },
    }
}

/// Convenience used by reports: the error as text, without failing.
pub fn query_or_error(path: &Path) -> Result<VolumeInfo, String> {
    query(path).map_err(|e| format!("{} (code {})", e.message().trim_end(), error::os_code(&e)))
}
