use personal_rns::routing::links::resources::{
    map_hash, resource_sdu, SaltNonce, COLLISION_GUARD_SIZE, MAP_HASH_LEN,
};
use personal_rns::wire::BROADCAST_MTU;

struct Xorshift(u64);

impl Xorshift {
    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn fill(&mut self, buf: &mut [u8]) {
        for chunk in buf.chunks_mut(8) {
            let bytes = self.next_u64().to_le_bytes();
            chunk.copy_from_slice(&bytes[..chunk.len()]);
        }
    }
}

fn main() {
    let sdu = resource_sdu(BROADCAST_MTU);
    let mut rng = Xorshift(0x9E37_79B9_7F4A_7C15);
    println!(
        "sdu={sdu}B  name={MAP_HASH_LEN}B (32-bit)  guard_window={COLLISION_GUARD_SIZE} parts"
    );
    println!(
        "{:<10} {:<8} {:<10} {:<11} {:<12}",
        "size", "parts", "builds", "collisions", "reroll_rate"
    );
    for &size in &[20_000usize, 70_000, 120_000, 1_048_575] {
        let parts = size.div_ceil(sdu);
        let builds = (30_000_000 / parts).max(2_000);
        let mut sealed = std::vec![0u8; parts * sdu];
        rng.fill(&mut sealed);
        let mut names: std::vec::Vec<[u8; MAP_HASH_LEN]> = std::vec::Vec::with_capacity(parts);
        let mut collisions = 0u64;
        for _ in 0..builds {
            let mut nonce = [0u8; 4];
            rng.fill(&mut nonce);
            let salt = SaltNonce::new(nonce);
            names.clear();
            for part in sealed.chunks(sdu) {
                names.push(map_hash(part, &salt));
            }
            let mut collided = false;
            'scan: for i in 1..names.len() {
                let lo = i.saturating_sub(COLLISION_GUARD_SIZE);
                for j in lo..i {
                    if names[j] == names[i] {
                        collided = true;
                        break 'scan;
                    }
                }
            }
            if collided {
                collisions += 1;
            }
        }
        println!(
            "{:<10} {:<8} {:<10} {:<11} {:.6}%",
            size,
            parts,
            builds,
            collisions,
            100.0 * collisions as f64 / builds as f64
        );
    }
}
