#![cfg_attr(target_arch = "bpf", no_std, no_builtins)]

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    unsafe { core::hint::unreachable_unchecked() }
}

pub fn sol_log_data(data: &[&[u8]]) {
    let sol_log_data: unsafe extern "C" fn(data: *const u8, len: u64) = unsafe { core::mem::transmute(0x7317b434_usize) };
    unsafe { sol_log_data(data.as_ptr() as *const u8, data.len() as u64) }
}

#[unsafe(no_mangle)]
pub fn entrypoint(input: *mut u8) -> u64 {
    let x: u128 = unsafe { (*(input.add(0x0010) as *const u128)) * 0x03 };
    sol_log_data(&[x.to_le_bytes().as_ref()]);
    0
}

#[cfg(test)]
mod tests {
    use mollusk_svm::{Mollusk, result::Check};
    use solana_instruction::Instruction;

    #[test]
    pub fn hello_world() {
        let mollusk = Mollusk::new(&[2u8;32].into(), "target/bpfel-unknown-none/release/libupstream_u128_test");
        mollusk.process_and_validate_instruction(&Instruction {
            program_id: [2u8;32].into(),
            accounts: vec![],
            data: vec![0x0a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00 ]
        }, &vec![], &[
            Check::success()
        ]);
    }
}
