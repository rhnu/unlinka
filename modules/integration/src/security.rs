//! Protected current-user directories and handle-bound Authenticode verification.

use std::{
    fs::{File, OpenOptions},
    io,
    mem::size_of,
    os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
    path::Path,
};

use windows::{
    Win32::{
        Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, HANDLE, HLOCAL, HWND, LocalFree},
        Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                SE_FILE_OBJECT, SetSecurityInfo,
            },
            Cryptography::{
                CERT_NAME_STR_REVERSE_FLAG, CERT_STRING_TYPE, CERT_X500_NAME_STR, CertNameToStrW,
                X509_ASN_ENCODING,
            },
            DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, GetTokenInformation,
            PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES,
            TOKEN_QUERY, TOKEN_USER, TokenUser,
            WinTrust::{
                WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0,
                WINTRUST_FILE_INFO, WTD_CHOICE_FILE, WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT,
                WTD_REVOKE_WHOLECHAIN, WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UI_NONE,
                WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData, WinVerifyTrustEx,
            },
        },
        Storage::FileSystem::CreateDirectoryW,
        System::Threading::{GetCurrentProcess, OpenProcessToken},
    },
    core::{GUID, HRESULT, PWSTR},
};

use crate::{check_file, check_no_reparse, invalid, pcw, wide, win_error};

struct Token(HANDLE);

impl Drop for Token {
    fn drop(&mut self) {
        // SAFETY: this handle came from a successful OpenProcessToken and is owned exactly once.
        drop(unsafe { CloseHandle(self.0) });
    }
}

struct LocalAllocation(HLOCAL);

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        // SAFETY: Windows allocated this SID string or security descriptor with LocalAlloc.
        let _remaining_allocation = unsafe { LocalFree(Some(self.0)) };
    }
}

fn structure_size<T>() -> io::Result<u32> {
    u32::try_from(size_of::<T>()).map_err(|error| invalid(error.to_string()))
}

pub(crate) fn user_sid() -> io::Result<String> {
    // SAFETY: retrieving the current process pseudo-handle has no pointer preconditions.
    let process = unsafe { GetCurrentProcess() };
    let mut handle = HANDLE::default();
    // SAFETY: the output slot is writable and the requested token access is read-only.
    unsafe { OpenProcessToken(process, TOKEN_QUERY, &raw mut handle) }.map_err(win_error)?;
    let token = Token(handle);
    let mut length = 0;
    // SAFETY: the null first buffer asks Windows only for the necessary byte count.
    let sizing = unsafe { GetTokenInformation(token.0, TokenUser, None, 0, &raw mut length) };
    if sizing.is_ok() || length < structure_size::<TOKEN_USER>()? || length > 65_536 {
        return Err(invalid("invalid token information size"));
    }
    let mut buffer = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
    // SAFETY: usize storage supplies TOKEN_USER alignment and at least the requested byte count.
    unsafe {
        GetTokenInformation(
            token.0,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            length,
            &raw mut length,
        )
    }
    .map_err(win_error)?;
    // SAFETY: the successful call initialized a TOKEN_USER within this aligned, still-owned buffer.
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    let mut string = PWSTR::null();
    // SAFETY: the token's SID remains in the live buffer; Windows initializes the output allocation.
    unsafe { ConvertSidToStringSidW(user.User.Sid, &raw mut string) }.map_err(win_error)?;
    let _allocation = LocalAllocation(HLOCAL(string.0.cast()));
    // SAFETY: the converter returned a NUL-terminated UTF-16 SID string owned by the allocation guard.
    unsafe { string.to_string() }.map_err(|error| invalid(error.to_string()))
}

/// Protects a directory by handle and retains a non-delete-shared handle during its use.
pub(crate) fn private_dir(path: &Path) -> io::Result<File> {
    check_no_reparse(path)?;
    let sddl = wide(format!("D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;{})", user_sid()?));
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: the SDDL is terminated and the writable output receives a LocalAlloc descriptor.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            pcw(&sddl),
            1,
            &raw mut descriptor,
            None,
        )
    }
    .map_err(win_error)?;
    let _allocation = LocalAllocation(HLOCAL(descriptor.0));
    let attributes = SECURITY_ATTRIBUTES {
        nLength: structure_size::<SECURITY_ATTRIBUTES>()?,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: false.into(),
    };
    let name = wide(path);
    // SAFETY: the path and security descriptor remain owned for the synchronous creation call.
    if let Err(error) = unsafe { CreateDirectoryW(pcw(&name), Some(&raw const attributes)) }
        && error.code() != HRESULT::from_win32(ERROR_ALREADY_EXISTS.0)
    {
        return Err(win_error(error));
    }
    let file = OpenOptions::new()
        .access_mode(0x0002_0000 | 0x0004_0000 | 0x80)
        .share_mode(3)
        .custom_flags(0x0200_0000 | 0x0020_0000)
        .open(path)?;
    check_file(&file)?;
    if !file.metadata()?.is_dir() {
        return Err(invalid("request root is not a directory"));
    }
    let mut present = false.into();
    let mut defaulted = false.into();
    let mut acl = std::ptr::null_mut();
    // SAFETY: the live converted descriptor supplies the ACL, and all output slots are writable.
    unsafe {
        GetSecurityDescriptorDacl(
            descriptor,
            &raw mut present,
            &raw mut acl,
            &raw mut defaulted,
        )
    }
    .map_err(win_error)?;
    // SAFETY: the no-reparse directory handle is owned and the descriptor's ACL remains live.
    unsafe {
        SetSecurityInfo(
            HANDLE(file.as_raw_handle()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(acl),
            None,
        )
    }
    .ok()
    .map_err(win_error)?;
    Ok(file)
}

struct TrustState {
    action: GUID,
    data: WINTRUST_DATA,
}

impl Drop for TrustState {
    fn drop(&mut self) {
        self.data.dwStateAction = WTD_STATEACTION_CLOSE;
        // SAFETY: this structure owns the provider state; CLOSE releases it before input data expires.
        let _close_status = unsafe {
            WinVerifyTrustEx(
                HWND(std::ptr::null_mut()),
                &raw mut self.action,
                &raw mut self.data,
            )
        };
    }
}

/// Verifies the certificate chain without UI, retaining the same no-reparse file handle.
pub(crate) fn signed_publisher(path: &Path) -> io::Result<String> {
    check_no_reparse(path)?;
    let file = OpenOptions::new()
        .read(true)
        .share_mode(1)
        .custom_flags(0x0020_0000)
        .open(path)?;
    check_file(&file)?;
    let name = wide(path);
    let mut file_info = WINTRUST_FILE_INFO {
        cbStruct: structure_size::<WINTRUST_FILE_INFO>()?,
        pcwszFilePath: pcw(&name),
        hFile: HANDLE(file.as_raw_handle()),
        ..Default::default()
    };
    let mut trust = TrustState {
        action: WINTRUST_ACTION_GENERIC_VERIFY_V2,
        data: WINTRUST_DATA {
            cbStruct: structure_size::<WINTRUST_DATA>()?,
            dwUIChoice: WTD_UI_NONE,
            fdwRevocationChecks: WTD_REVOKE_WHOLECHAIN,
            dwUnionChoice: WTD_CHOICE_FILE,
            Anonymous: WINTRUST_DATA_0 {
                pFile: &raw mut file_info,
            },
            dwStateAction: WTD_STATEACTION_VERIFY,
            dwProvFlags: WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT,
            ..Default::default()
        },
    };
    // SAFETY: the file, file-info structure and terminated name outlive provider verification and CLOSE.
    let code = unsafe {
        WinVerifyTrustEx(
            HWND(std::ptr::null_mut()),
            &raw mut trust.action,
            &raw mut trust.data,
        )
    };
    if code != 0 {
        return Err(io::Error::other(format!(
            "signature verification failed HRESULT {code:#010x}"
        )));
    }
    signer_subject(&trust.data)
}

fn signer_subject(data: &WINTRUST_DATA) -> io::Result<String> {
    // SAFETY: successful verification initialized the provider handle, retained until TrustState drops.
    let provider = unsafe { WTHelperProvDataFromStateData(data.hWVTStateData) };
    if provider.is_null() {
        return Err(invalid("missing signature provider"));
    }
    // SAFETY: provider is live; signer index zero identifies the verified primary signature.
    let signer = unsafe { WTHelperGetProvSignerFromChain(provider, 0, false, 0) };
    if signer.is_null() {
        return Err(invalid("missing signing certificate"));
    }
    // SAFETY: the provider returned a valid CRYPT_PROVIDER_SGNR until its state is closed.
    let signer = unsafe { &*signer };
    if signer.csCertChain == 0 || signer.pasCertChain.is_null() {
        return Err(invalid("missing signing certificate"));
    }
    // SAFETY: csCertChain proves at least one provider-owned certificate entry exists.
    let first = unsafe { &*signer.pasCertChain };
    if first.pCert.is_null() {
        return Err(invalid("invalid signing certificate"));
    }
    // SAFETY: the first certificate context remains owned by the live trust provider.
    let certificate = unsafe { &*first.pCert };
    if certificate.pCertInfo.is_null() {
        return Err(invalid("invalid signing certificate"));
    }
    // SAFETY: CERT_CONTEXT supplies a live certificate-info structure until trust CLOSE.
    let information = unsafe { &*certificate.pCertInfo };
    let flags = CERT_STRING_TYPE(CERT_X500_NAME_STR.0 | CERT_NAME_STR_REVERSE_FLAG);
    // SAFETY: the subject belongs to this verified certificate; a null output asks for its capacity.
    let length = unsafe {
        CertNameToStrW(
            X509_ASN_ENCODING,
            &raw const information.Subject,
            flags,
            None,
        )
    };
    if length == 0 || length > 65_536 {
        return Err(invalid("invalid signing subject length"));
    }
    let mut buffer = vec![0; length as usize];
    // SAFETY: the output capacity matches the provider's requested number of UTF-16 units.
    let written = unsafe {
        CertNameToStrW(
            X509_ASN_ENCODING,
            &raw const information.Subject,
            flags,
            Some(&mut buffer),
        )
    };
    if written != length {
        return Err(invalid("signing subject conversion failed"));
    }
    String::from_utf16(&buffer[..buffer.len() - 1]).map_err(|error| invalid(error.to_string()))
}
