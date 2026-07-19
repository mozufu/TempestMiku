use sha2::{Digest, Sha256};

use crate::{HostError, Result};

pub(super) const POLICY_VERSION: &str = "developer_v1";

const BPF_LD_W_ABS: u16 = 0x20;
const BPF_JMP_JEQ_K: u16 = 0x15;
const BPF_JMP_JSET_K: u16 = 0x45;
const BPF_RET_K: u16 = 0x06;
const SECCOMP_RET_KILL_PROCESS: u32 = 0x8000_0000;
const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;
const SECCOMP_DATA_NR_OFFSET: u32 = 0;
const SECCOMP_DATA_ARCH_OFFSET: u32 = 4;
const SECCOMP_DATA_ARG0_OFFSET: u32 = 16;
const EPERM: u32 = 1;
const ENOSYS: u32 = 38;

// Ordinary fork/thread creation remains available. Only namespace-creating clone flags are
// rejected. clone3 cannot be inspected safely from classic BPF because its flags live behind a
// userspace pointer, so it is reported as unavailable and libc can fall back to clone.
const CLONE_NAMESPACE_FLAGS: u32 = 0x0000_0080
    | 0x0002_0000
    | 0x0200_0000
    | 0x0400_0000
    | 0x0800_0000
    | 0x1000_0000
    | 0x2000_0000
    | 0x4000_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LinuxAuditArch {
    X86_64,
    Aarch64,
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl LinuxAuditArch {
    pub(super) fn current() -> Result<Self> {
        match std::env::consts::ARCH {
            "x86_64" => Ok(Self::X86_64),
            "aarch64" => Ok(Self::Aarch64),
            arch => Err(HostError::CapabilityDenied(format!(
                "linux_hardened_v1 has no developer_v1 seccomp policy for architecture {arch}"
            ))),
        }
    }

    pub(super) const fn name(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
        }
    }

    const fn audit_value(self) -> u32 {
        match self {
            Self::X86_64 => 0xc000_003e,
            Self::Aarch64 => 0xc000_00b7,
        }
    }

    const fn clone_syscall(self) -> u32 {
        match self {
            Self::X86_64 => 56,
            Self::Aarch64 => 220,
        }
    }

    const fn clone3_syscall(self) -> u32 {
        435
    }

    const fn denied_syscalls(self) -> &'static [u32] {
        match self {
            Self::X86_64 => &[
                // mount and filesystem topology
                155, 165, 166, 428, 429, 430, 431, 432, 433, 442, // namespaces
                272, 308, // kernel attack surfaces and cross-process inspection
                321, 298, 248, 249, 250, 175, 176, 313, 246, 320, 169, 167, 168, 101, 310, 311,
                323,
            ],
            Self::Aarch64 => &[
                // mount and filesystem topology
                41, 40, 39, 428, 429, 430, 431, 432, 433, 442, // namespaces
                97, 268, // kernel attack surfaces and cross-process inspection
                280, 241, 217, 218, 219, 105, 106, 273, 104, 294, 142, 224, 225, 117, 270, 271,
                282,
            ],
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Instruction {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}

impl Instruction {
    const fn statement(code: u16, k: u32) -> Self {
        Self {
            code,
            jt: 0,
            jf: 0,
            k,
        }
    }

    const fn jump(code: u16, k: u32, jt: u8, jf: u8) -> Self {
        Self { code, jt, jf, k }
    }

    fn append_bytes(self, bytes: &mut Vec<u8>) {
        bytes.extend_from_slice(&self.code.to_ne_bytes());
        bytes.push(self.jt);
        bytes.push(self.jf);
        bytes.extend_from_slice(&self.k.to_ne_bytes());
    }
}

pub(super) fn developer_v1_policy(arch: LinuxAuditArch) -> Vec<u8> {
    let mut instructions = vec![
        Instruction::statement(BPF_LD_W_ABS, SECCOMP_DATA_ARCH_OFFSET),
        Instruction::jump(BPF_JMP_JEQ_K, arch.audit_value(), 1, 0),
        Instruction::statement(BPF_RET_K, SECCOMP_RET_KILL_PROCESS),
        Instruction::statement(BPF_LD_W_ABS, SECCOMP_DATA_NR_OFFSET),
        // glibc retries clone when clone3 reports ENOSYS. This keeps normal threads and cargo
        // subprocesses working while preventing opaque clone3 namespace flags.
        Instruction::jump(BPF_JMP_JEQ_K, arch.clone3_syscall(), 0, 1),
        Instruction::statement(BPF_RET_K, SECCOMP_RET_ERRNO | ENOSYS),
        // Skip four instructions when this is not clone. For clone, inspect only namespace flags,
        // then reload the syscall number before continuing the linear deny list.
        Instruction::jump(BPF_JMP_JEQ_K, arch.clone_syscall(), 0, 4),
        Instruction::statement(BPF_LD_W_ABS, SECCOMP_DATA_ARG0_OFFSET),
        Instruction::jump(BPF_JMP_JSET_K, CLONE_NAMESPACE_FLAGS, 0, 1),
        Instruction::statement(BPF_RET_K, SECCOMP_RET_ERRNO | EPERM),
        Instruction::statement(BPF_LD_W_ABS, SECCOMP_DATA_NR_OFFSET),
    ];
    for syscall in arch.denied_syscalls() {
        instructions.push(Instruction::jump(BPF_JMP_JEQ_K, *syscall, 0, 1));
        instructions.push(Instruction::statement(BPF_RET_K, SECCOMP_RET_ERRNO | EPERM));
    }
    instructions.push(Instruction::statement(BPF_RET_K, SECCOMP_RET_ALLOW));

    let mut bytes = Vec::with_capacity(instructions.len() * 8);
    for instruction in instructions {
        instruction.append_bytes(&mut bytes);
    }
    bytes
}

pub(super) fn policy_sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(target_os = "linux")]
pub(super) fn sealed_policy_file(bytes: &[u8]) -> Result<std::fs::File> {
    use std::{
        ffi::CString,
        io::{Seek, SeekFrom, Write},
        os::fd::FromRawFd,
    };

    let name = CString::new("tempestmiku-seccomp-developer-v1").expect("static memfd name");
    // SAFETY: name is a live C string and these memfd flags require no further arguments.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_memfd_create,
            name.as_ptr(),
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        ) as libc::c_int
    };
    if fd < 0 {
        return Err(HostError::CapabilityDenied(format!(
            "linux_hardened_v1 cannot create sealed seccomp policy fd: {}",
            std::io::Error::last_os_error()
        )));
    }
    // SAFETY: memfd_create returned a new owned descriptor.
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    file.write_all(bytes).map_err(|error| {
        HostError::CapabilityDenied(format!(
            "linux_hardened_v1 cannot write seccomp policy: {error}"
        ))
    })?;
    file.seek(SeekFrom::Start(0)).map_err(|error| {
        HostError::CapabilityDenied(format!(
            "linux_hardened_v1 cannot rewind seccomp policy: {error}"
        ))
    })?;
    let seals = libc::F_SEAL_SEAL | libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_WRITE;
    // SAFETY: F_ADD_SEALS only mutates the owned memfd's seal mask.
    if unsafe { libc::fcntl(fd, libc::F_ADD_SEALS, seals) } == -1 {
        return Err(HostError::CapabilityDenied(format!(
            "linux_hardened_v1 cannot seal seccomp policy: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn instructions(bytes: &[u8]) -> Vec<(u16, u8, u8, u32)> {
        assert_eq!(bytes.len() % 8, 0);
        bytes
            .chunks_exact(8)
            .map(|chunk| {
                (
                    u16::from_ne_bytes([chunk[0], chunk[1]]),
                    chunk[2],
                    chunk[3],
                    u32::from_ne_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]),
                )
            })
            .collect()
    }

    #[test]
    fn developer_v1_is_versioned_arch_aware_and_deterministic() {
        let x86 = developer_v1_policy(LinuxAuditArch::X86_64);
        let arm = developer_v1_policy(LinuxAuditArch::Aarch64);
        assert_ne!(x86, arm);
        assert_eq!(x86, developer_v1_policy(LinuxAuditArch::X86_64));
        assert_eq!(arm, developer_v1_policy(LinuxAuditArch::Aarch64));
        assert_eq!(POLICY_VERSION, "developer_v1");
        assert_eq!(x86.len() % 8, 0);
        assert_eq!(arm.len() % 8, 0);
        assert_ne!(policy_sha256(&x86), policy_sha256(&arm));
        assert_eq!(
            policy_sha256(&x86),
            "5f5a85e74abb372634d8a0bc05bc9c29aaf361803c7b2eaed49af11d3ee22487"
        );
        assert_eq!(
            policy_sha256(&arm),
            "a0199be3c1a1aa40b387946bd05ae72771f44119074e84cad4b118b565a9d60a"
        );
    }

    #[test]
    fn developer_v1_kills_wrong_arch_and_blocks_required_attack_surfaces() {
        for arch in [LinuxAuditArch::X86_64, LinuxAuditArch::Aarch64] {
            let policy = instructions(&developer_v1_policy(arch));
            assert_eq!(
                &policy[..4],
                &[
                    (BPF_LD_W_ABS, 0, 0, SECCOMP_DATA_ARCH_OFFSET),
                    (BPF_JMP_JEQ_K, 1, 0, arch.audit_value()),
                    (BPF_RET_K, 0, 0, SECCOMP_RET_KILL_PROCESS),
                    (BPF_LD_W_ABS, 0, 0, SECCOMP_DATA_NR_OFFSET),
                ]
            );
            for syscall in arch.denied_syscalls() {
                assert!(policy.windows(2).any(|pair| {
                    pair[0] == (BPF_JMP_JEQ_K, 0, 1, *syscall)
                        && pair[1] == (BPF_RET_K, 0, 0, SECCOMP_RET_ERRNO | EPERM)
                }));
            }
            assert!(policy.contains(&(BPF_JMP_JSET_K, 0, 1, CLONE_NAMESPACE_FLAGS)));
            assert_eq!(policy.last(), Some(&(BPF_RET_K, 0, 0, SECCOMP_RET_ALLOW)));
        }
    }
}
