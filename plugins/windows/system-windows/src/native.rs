//! The only unsafe Shell adapter. All destructive targets come from a fresh Recycle Bin enumeration.
use serde::{Deserialize, Serialize};
use spaceprune_plugin_api::OperationContext;
use std::{fmt, io, path::PathBuf, process::Command};
use windows::{
    Win32::{
        Foundation::{FILETIME, PROPERTYKEY},
        System::Com::{
            CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE,
            CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
        },
        UI::Shell::{PropertiesSystem::PSGetPropertyKeyFromName, *},
    },
    core::{Interface, PWSTR, w},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RecycleItem {
    pub(super) owner: String,
    pub(super) identity: String,
    pub(super) name: String,
    pub(super) location: String,
    pub(super) bytes: Option<u64>,
    pub(super) deleted_at: String,
    pub(super) modified_at: String,
}
#[derive(Debug)]
pub(super) struct Failure {
    pub(super) started: bool,
    pub(super) message: String,
}
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
fn before(error: impl fmt::Display) -> Failure {
    Failure {
        started: false,
        message: error.to_string(),
    }
}
fn after(error: impl fmt::Display) -> Failure {
    Failure {
        started: true,
        message: error.to_string(),
    }
}
struct Apartment;
impl Apartment {
    fn open() -> Result<Self, Failure> {
        unsafe {
            CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE)
                .ok()
                .map_err(before)?;
        }
        Ok(Self)
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}
struct ShellText(PWSTR);
impl Drop for ShellText {
    fn drop(&mut self) {
        unsafe {
            CoTaskMemFree(Some(self.0.0.cast()));
        }
    }
}
fn text(pointer: PWSTR) -> Result<String, Failure> {
    let value = ShellText(pointer);
    unsafe { value.0.to_string().map_err(before) }
}
fn ticks(value: FILETIME) -> String {
    ((u64::from(value.dwHighDateTime) << 32) | u64::from(value.dwLowDateTime)).to_string()
}
fn record(item: &IShellItem, owner: &str) -> Result<RecycleItem, Failure> {
    let properties: IShellItem2 = item.cast().map_err(before)?;
    unsafe {
        let mut size = PROPERTYKEY::default();
        PSGetPropertyKeyFromName(w!("System.Size"), &mut size).map_err(before)?;
        let mut deleted = PROPERTYKEY::default();
        PSGetPropertyKeyFromName(w!("System.Recycle.DateDeleted"), &mut deleted).map_err(before)?;
        let mut modified = PROPERTYKEY::default();
        PSGetPropertyKeyFromName(w!("System.DateModified"), &mut modified).map_err(before)?;
        let original = text(item.GetDisplayName(SIGDN_NORMALDISPLAY).map_err(before)?)?;
        let name = std::path::Path::new(&original)
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| original.clone());
        Ok(RecycleItem {
            owner: owner.into(),
            identity: text(
                item.GetDisplayName(SIGDN_DESKTOPABSOLUTEPARSING)
                    .map_err(before)?,
            )?,
            name,
            location: original,
            bytes: properties.GetUInt64(&size).ok(),
            deleted_at: ticks(properties.GetFileTime(&deleted).map_err(before)?),
            modified_at: ticks(properties.GetFileTime(&modified).map_err(before)?),
        })
    }
}
fn enumerated(context: &OperationContext) -> Result<Vec<(RecycleItem, IShellItem)>, Failure> {
    let owner = spaceprune_windows_elevation::current_user_sid()
        .map_err(|_| before("Windows user identity unavailable"))?;
    let mut result = Vec::new();
    unsafe {
        let bin: IShellItem =
            SHGetKnownFolderItem(&FOLDERID_RecycleBinFolder, KF_FLAG_DEFAULT, None)
                .map_err(before)?;
        let items: IEnumShellItems = bin.BindToHandler(None, &BHID_EnumItems).map_err(before)?;
        loop {
            context.check().map_err(before)?;
            let mut next = [None];
            let mut fetched = 0;
            items.Next(&mut next, Some(&mut fetched)).map_err(before)?;
            if fetched == 0 {
                break;
            }
            if result.len() >= 50_000 {
                return Err(before("Recycle Bin inventory exceeds the inspection limit"));
            }
            let item = next[0]
                .take()
                .ok_or_else(|| before("Invalid Shell enumeration"))?;
            result.push((record(&item, &owner)?, item));
        }
    }
    Ok(result)
}
pub(super) fn recycle_items(context: &OperationContext) -> Result<Vec<RecycleItem>, Failure> {
    let _apartment = Apartment::open()?;
    Ok(enumerated(context)?
        .into_iter()
        .map(|(record, _)| record)
        .collect())
}
pub(super) fn delete_recycle_item(
    selected: &RecycleItem,
    context: &OperationContext,
) -> Result<(), Failure> {
    let _apartment = Apartment::open()?;
    // Never reconstruct a filesystem target from client-supplied opaque data or clear the whole bin.
    let item = enumerated(context)?
        .into_iter()
        .find(|(record, _)| record == selected)
        .map(|(_, item)| item)
        .ok_or_else(|| before("Selected recycle item changed or was restored; scan again"))?;
    context.check().map_err(before)?;
    unsafe {
        let operation: IFileOperation =
            CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER).map_err(before)?;
        operation
            .SetOperationFlags(
                FOF_SILENT
                    | FOF_NOCONFIRMATION
                    | FOF_NOERRORUI
                    | FOF_NO_CONNECTED_ELEMENTS
                    | FILEOPERATION_FLAGS(FOFX_EARLYFAILURE.0 | FOFX_NOCOPYHOOKS.0),
            )
            .map_err(before)?;
        operation.DeleteItem(&item, None).map_err(before)?;
        context.check().map_err(before)?;
        operation.PerformOperations().map_err(after)?;
        if operation
            .GetAnyOperationsAborted()
            .map_err(after)?
            .as_bool()
        {
            return Err(after(
                "Windows did not finish deleting the selected recycle item",
            ));
        }
    }
    if enumerated(context)
        .map_err(|error| after(error.message))?
        .iter()
        .any(|(record, _)| record.identity == selected.identity)
    {
        return Err(after(
            "Windows has not confirmed removal of the selected recycle item",
        ));
    }
    Ok(())
}
pub(super) fn dism_path() -> io::Result<PathBuf> {
    let locations = spaceprune_windows_elevation::SystemLocations::query()
        .ok_or_else(|| io::Error::other("Windows system location unavailable"))?;
    let path = locations
        .windows_directory()
        .join("System32")
        .join("Dism.exe");
    let metadata = std::fs::symlink_metadata(&path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    Ok(path)
}
pub(super) fn cleanup_components(context: &OperationContext) -> Result<(), Failure> {
    if !spaceprune_windows_elevation::is_elevated() {
        return Err(before("Windows servicing requires system authorization"));
    }
    context.check().map_err(before)?;
    let path = dism_path().map_err(before)?;
    let mut command = Command::new(&path);
    command.current_dir(path.parent().expect("system directory"));
    // Do not accept caller-supplied arguments, stop update services, remove WinSxS files or use ResetBase.
    command.args([
        "/Online",
        "/Cleanup-Image",
        "/StartComponentCleanup",
        "/NoRestart",
        "/English",
    ]);
    let output = spaceprune_access::process::capture(command, context).map_err(after)?;
    if output.status.success() || output.status.code() == Some(3010) {
        Ok(())
    } else {
        Err(after(format!(
            "Windows servicing returned {}",
            output.status
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::core::PCWSTR;

    #[test]
    #[ignore = "Native Windows acceptance; recycles and deletes only uniquely named disposable fixtures"]
    fn deletes_only_selected_fixture_and_rejects_changed_identity() {
        let directory = tempfile::tempdir().unwrap();
        let suffix = directory.path().file_name().unwrap().to_string_lossy();
        let names = [
            format!("SpacePrune-disposable-{suffix}-one.txt"),
            format!("SpacePrune-disposable-{suffix}-two.txt"),
        ];
        let context = OperationContext::default();
        let _apartment = Apartment::open().unwrap();
        for name in &names {
            let path = directory.path().join(name);
            std::fs::write(&path, b"disposable native recycle fixture").unwrap();
            let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            unsafe {
                let item: IShellItem =
                    SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None).unwrap();
                let operation: IFileOperation =
                    CoCreateInstance(&FileOperation, None, CLSCTX_INPROC_SERVER).unwrap();
                operation
                    .SetOperationFlags(
                        FOF_ALLOWUNDO | FOF_SILENT | FOF_NOCONFIRMATION | FOF_NOERRORUI,
                    )
                    .unwrap();
                operation.DeleteItem(&item, None).unwrap();
                operation.PerformOperations().unwrap();
                assert!(!operation.GetAnyOperationsAborted().unwrap().as_bool());
            }
            assert!(!path.exists());
        }
        let find = |name: &str| {
            recycle_items(&context)
                .unwrap()
                .into_iter()
                .find(|item| item.name == name)
                .expect("fixture is in the recycle bin")
        };
        let first = find(&names[0]);
        let second = find(&names[1]);
        let mut changed = first.clone();
        changed.deleted_at.push('0');
        assert!(!delete_recycle_item(&changed, &context).unwrap_err().started);
        delete_recycle_item(&first, &context).unwrap();
        assert_eq!(find(&names[1]), second);
        assert!(!delete_recycle_item(&first, &context).unwrap_err().started);
        delete_recycle_item(&second, &context).unwrap();
    }
    use std::os::windows::ffi::OsStrExt;
}
