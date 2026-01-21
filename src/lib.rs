#![cfg_attr(target_arch = "bpf", no_std)]

#[cfg(target_arch = "bpf")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    unsafe { core::hint::unreachable_unchecked() }
}

#[unsafe(no_mangle)]
pub fn entrypoint(i: *mut u8) -> u64 {
    let mut a = unsafe { *(i.add(0x0010) as *const u128) };
    let b = unsafe { *((i.add(0x0010) as *const u128).wrapping_add(1)) };
    
    for _ in 0..10000 {
        // reassign a to avoid multiply being optimized away
        a = a * b;
    }
    
    (a >> 64) as u64
}

#[cfg(test)]
mod tests {
    use mollusk_svm::{Mollusk, result::Check};
    use solana_instruction::Instruction;

    const PROGRAM_ID: [u8; 32] = [0x02; 32];

    #[test]
    pub fn test() {
        let mollusk = Mollusk::new(&PROGRAM_ID.into(), // 
            "target/bpfel-unknown-none/release/libupstream_u128_test");
        let input_data : [i128; 2] = [10, 20];
        let instruction = solana_instruction::Instruction {
            program_id: PROGRAM_ID.into(),
            accounts: vec![],
            data: input_data.iter().flat_map(|x| x.to_le_bytes()).collect(),
        };
        mollusk.process_and_validate_instruction(&instruction, &[], &[Check::success()]);
    }
}
