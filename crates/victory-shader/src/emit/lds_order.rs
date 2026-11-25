//! Whether LDS accesses inside one lane-branch region of a console wave64 program run as
//! two host waves of 32 can do without a barrier between them (see `lower::lds_meetings`).
//!
//! The console runs a wave's 64 lanes in step, so inside a region (blocks not every lane
//! runs, between two that all do) a read sees every earlier write of the wave. Two host
//! waves cannot meet there, so a read in one half could miss a write of the other. That
//! cannot happen when no thread in the region reads or writes a dword another thread
//! writes, which this module proves from the addresses where it can:
//!
//! - every access in the region runs in one thread at most (it is under a compare of a
//!   value unique to each thread, its thread index, with one the whole group shares), or
//! - for each pair of accesses of which one writes (two atomics excepted: their order does
//!   not change the memory, only which thread gets which old value, which the console does
//!   not fix across waves either), the bytes the two can touch over all threads are apart,
//!   or both are the same thread's own slot: the same multiple of the thread index plus
//!   the same shared base, with the slots of two threads apart.
//!
//! Addresses are followed as sums of the thread index's components times constants, of
//! scalar values (shared by the group) and of a constant, through moves, adds, subtracts,
//! shifts and multiplies by constants. Anything else is not proven, and the region keeps
//! its refusal.

use std::collections::HashMap;

use crate::gfx10::{Fields, Operand};
use crate::ir::{Inst, Program, Reg, Value};
use crate::structure::{Atom, Bool, Structured};

/// An address as a linear form: `tid · thread index + Σ coefficient · scalar + k`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Lin {
    tid: [i64; 3],
    /// Scalar values and their coefficients, sorted, without zeros.
    sym: Vec<(u32, i64)>,
    k: i64,
}

impl Lin {
    fn konst(k: i64) -> Self {
        Lin { tid: [0; 3], sym: Vec::new(), k }
    }

    fn add(&self, o: &Lin, sign: i64) -> Lin {
        let mut sym: HashMap<u32, i64> = self.sym.iter().copied().collect();
        for &(v, c) in &o.sym {
            *sym.entry(v).or_default() += sign * c;
        }
        let mut sym: Vec<(u32, i64)> = sym.into_iter().filter(|s| s.1 != 0).collect();
        sym.sort_unstable();
        Lin {
            tid: [0, 1, 2].map(|i| self.tid[i] + sign * o.tid[i]),
            sym,
            k: self.k.wrapping_add(sign.wrapping_mul(o.k)),
        }
    }

    fn scale(&self, m: i64) -> Lin {
        Lin {
            tid: self.tid.map(|t| t * m),
            sym: self.sym.iter().map(|&(v, c)| (v, c * m)).filter(|s| s.1 != 0).collect(),
            k: self.k.wrapping_mul(m),
        }
    }

    fn uniform(&self) -> bool {
        self.tid == [0; 3]
    }
}

/// Linear forms of VGPR values, with the instructions that write them.
pub(super) struct Addresses<'a> {
    p: &'a Program,
    defs: HashMap<Value, &'a Inst>,
    /// The thread index components on entry (v0, v1, v2), by value.
    tid: HashMap<Value, usize>,
    memo: HashMap<Value, Option<Lin>>,
}

impl<'a> Addresses<'a> {
    pub(super) fn new(p: &'a Program, tid_vgprs: u8) -> Self {
        let mut defs = HashMap::new();
        for inst in p.blocks.iter().flat_map(|b| &b.insts) {
            for &(_, w) in &inst.writes {
                defs.insert(w, inst);
            }
        }
        let tid = p
            .inputs
            .iter()
            .filter_map(|&(r, v)| match r {
                Reg::V(i) if i < tid_vgprs.min(3) => Some((v, usize::from(i))),
                _ => None,
            })
            .collect();
        Addresses { p, defs, tid, memo: HashMap::new() }
    }

    /// The linear form of an operand of `inst`, if it has one.
    fn operand(&mut self, inst: &Inst, op: &Operand, depth: u32) -> Option<Lin> {
        match *op {
            Operand::Const(c) => crate::resource::inline(c).map(|k| Lin::konst(i64::from(k as i32))),
            Operand::Literal(x) => Some(Lin::konst(i64::from(x as i32))),
            Operand::Scalar { code, .. } => {
                let v = inst.read(Reg::S(code))?;
                Some(Lin { tid: [0; 3], sym: vec![(v.0, 1)], k: 0 })
            }
            Operand::Vector { reg, .. } => {
                let v = inst.read(Reg::V(reg))?;
                self.value(v, depth)
            }
            _ => None,
        }
    }

    fn constant(&mut self, inst: &Inst, op: &Operand) -> Option<i64> {
        let l = self.operand(inst, op, 0)?;
        (l.uniform() && l.sym.is_empty()).then_some(l.k)
    }

    /// The linear form of VGPR value `v`.
    fn value(&mut self, v: Value, depth: u32) -> Option<Lin> {
        if let Some(&i) = self.tid.get(&v) {
            let mut t = [0; 3];
            t[i] = 1;
            return Some(Lin { tid: t, sym: Vec::new(), k: 0 });
        }
        if let Some(l) = self.memo.get(&v) {
            return l.clone();
        }
        if depth > 24 {
            return None;
        }
        let r = self.def(v, depth);
        self.memo.insert(v, r.clone());
        r
    }

    fn def(&mut self, v: Value, depth: u32) -> Option<Lin> {
        let inst = *self.defs.get(&v)?;
        let Fields::Valu(f) = &inst.fields else {
            return None;
        };
        if f.abs != 0 || f.neg != 0 || f.clamp || f.omod != 0 || f.sdwa.is_some() || f.dpp.is_some() || f.opsel != 0 {
            return None;
        }
        // the value is the instruction's first destination dword
        match f.dst {
            Some(Operand::Vector { reg, .. }) if inst.write(Reg::V(reg)) == Some(v) => {}
            _ => return None,
        }
        let src = |l: &mut Self, k: usize| -> Option<Lin> { l.operand(inst, f.src.get(k)?, depth + 1) };
        let k = |l: &mut Self, i: usize| -> Option<i64> { l.constant(inst, f.src.get(i)?) };
        Some(match inst.name() {
            "v_mov_b32" => src(self, 0)?,
            "v_add_nc_u32" | "v_add_u32" | "v_add_co_u32" => src(self, 0)?.add(&src(self, 1)?, 1),
            "v_sub_nc_u32" | "v_sub_co_u32" => src(self, 0)?.add(&src(self, 1)?, -1),
            "v_subrev_nc_u32" | "v_subrev_co_u32" => src(self, 1)?.add(&src(self, 0)?, -1),
            "v_lshlrev_b32" => {
                let sh = k(self, 0)?;
                if !(0..31).contains(&sh) {
                    return None;
                }
                src(self, 1)?.scale(1 << sh)
            }
            "v_lshl_add_u32" => {
                let sh = k(self, 1)?;
                if !(0..31).contains(&sh) {
                    return None;
                }
                src(self, 0)?.scale(1 << sh).add(&src(self, 2)?, 1)
            }
            "v_add_lshl_u32" => {
                let sh = k(self, 2)?;
                if !(0..31).contains(&sh) {
                    return None;
                }
                src(self, 0)?.add(&src(self, 1)?, 1).scale(1 << sh)
            }
            "v_mul_u32_u24" | "v_mul_lo_u32" | "v_mul_i32_i24" => match (k(self, 0), k(self, 1)) {
                (Some(m), _) => src(self, 1)?.scale(m),
                (_, Some(m)) => src(self, 0)?.scale(m),
                _ => return None,
            },
            "v_mad_u32_u24" | "v_mad_i32_i24" => {
                let prod = match (k(self, 0), k(self, 1)) {
                    (Some(m), _) => src(self, 1)?.scale(m),
                    (_, Some(m)) => src(self, 0)?.scale(m),
                    _ => return None,
                };
                prod.add(&src(self, 2)?, 1)
            }
            _ => return None,
        })
    }

    /// Whether VGPR or scalar `op` of `inst` is the same in every thread of the group.
    fn shared(&mut self, inst: &Inst, op: &Operand) -> bool {
        self.operand(inst, op, 0).is_some_and(|l| l.uniform())
    }

    /// Whether lane bit `v` (a compare's result) holds in one thread of the group at most:
    /// an equality of a value unique to each thread with one they all share.
    fn single_thread(&mut self, v: Value, threads: [u32; 3]) -> bool {
        let Some(inst) = self.defs.get(&v).copied() else {
            return false;
        };
        if !matches!(inst.name(), "v_cmp_eq_u32" | "v_cmp_eq_i32" | "v_cmpx_eq_u32" | "v_cmpx_eq_i32") {
            return false;
        }
        let Fields::Valu(f) = &inst.fields else {
            return false;
        };
        let (Some(a), Some(b)) = (f.src.first(), f.src.get(1)) else {
            return false;
        };
        let unique = |l: &mut Self, x: &Operand| l.operand(inst, x, 0).is_some_and(|lin| injective(&lin.tid, threads, 1));
        (unique(self, a) && self.shared(inst, b)) || (unique(self, b) && self.shared(inst, a))
    }

    /// The LDS bytes an instruction touches: its address's form and, relative to it,
    /// (offset, size) of each access; whether it writes, and whether it is an atomic.
    fn lds_access(&mut self, inst: &Inst) -> Option<(Lin, Vec<(i64, i64)>, bool, bool)> {
        let Fields::Ds { addr, offset0, offset1, gds, .. } = &inst.fields else {
            return None;
        };
        if *gds {
            return None;
        }
        let name = inst.name();
        let addr = match addr {
            Some(a) => self.operand(inst, a, 0)?,
            None => Lin::konst(0),
        };
        let off16 = i64::from(*offset1) << 8 | i64::from(*offset0);
        let size = |n: &str| -> Option<i64> {
            Some(if n.ends_with("_b128") {
                16
            } else if n.ends_with("_b96") {
                12
            } else if n.ends_with("_b64") || n.ends_with("_u64") || n.ends_with("_i64") || n.ends_with("_f64") {
                8
            } else if n.ends_with("_b32") || n.ends_with("_u32") || n.ends_with("_i32") || n.ends_with("_f32") {
                4
            } else if n.contains("_u16") || n.contains("_i16") || n.ends_with("_b16") || n.contains("d16") {
                2
            } else if n.contains("_u8") || n.contains("_i8") || n.ends_with("_b8") {
                1
            } else {
                return None;
            })
        };
        let two = name.starts_with("ds_read2") || name.starts_with("ds_write2");
        let st64 = name.contains("st64");
        let s = size(name)?;
        let parts = if two {
            let unit = s * if st64 { 64 } else { 1 };
            vec![(i64::from(*offset0) * unit, s), (i64::from(*offset1) * unit, s)]
        } else {
            vec![(off16, s)]
        };
        let read = name.starts_with("ds_read") || name.starts_with("ds_load");
        let write = name.starts_with("ds_write") || name.starts_with("ds_store");
        let atomic = !read && !write;
        Some((addr, parts, write || atomic, atomic))
    }
}

/// Whether `tid · thread index` gives every thread of a group of `threads` its own value, at
/// least `spacing` apart.
fn injective(tid: &[i64; 3], threads: [u32; 3], spacing: i64) -> bool {
    let mut dims: Vec<(i64, i64)> = (0..3)
        .filter(|&i| threads[i] > 1)
        .map(|i| (tid[i].abs(), i64::from(threads[i])))
        .collect();
    if dims.is_empty() {
        return true;
    }
    dims.sort_unstable();
    let mut covered = 0i64;
    for (c, n) in dims {
        if c < covered + spacing.max(1) {
            return false;
        }
        covered += c * (n - 1);
    }
    true
}

/// Whether `b` holds only where atom `a` does: with `a` false, `b` is false (every way to
/// true in the diagram goes through `a` true).
fn implies(st: &Structured, b: Bool, a: Atom) -> bool {
    match st.bools.node(b) {
        None => b == Bool::FALSE,
        Some((x, lo, _)) if x == a => lo == Bool::FALSE,
        Some((_, lo, hi)) => implies(st, lo, a) && implies(st, hi, a),
    }
}

/// Whether the LDS accesses `sites` (block, instruction) of one lane-branch region can run
/// without a barrier between the two host waves of a console wave.
pub(super) fn region_safe(
    a: &mut Addresses,
    st: &Structured,
    sites: &[(usize, usize)],
    threads: [u32; 3],
) -> bool {
    let p = a.p;
    // every access runs in one thread at most
    let single = sites.iter().all(|&(b, i)| {
        let cond = [Some(st.reach[b]), st.guards.get(b).and_then(|g| g.get(i).copied().flatten())];
        cond.into_iter().flatten().any(|c| {
            st.bools.support(c).into_iter().any(|atom| match atom {
                Atom::Lane(v) => implies(st, c, atom) && a.single_thread(v, threads),
                _ => false,
            })
        })
    });
    if single {
        return true;
    }
    let mut acc = Vec::new();
    for &(b, i) in sites {
        let inst = &p.blocks[b].insts[i];
        let name = inst.name();
        if name.starts_with("ds_swizzle") || name.contains("permute") {
            continue;
        }
        match a.lds_access(inst) {
            Some(x) => acc.push(x),
            None => return false,
        }
    }
    // the bytes an access can touch over all threads
    let bounds = |l: &Lin, parts: &[(i64, i64)]| -> (i64, i64) {
        let lo = parts.iter().map(|p| p.0).min().unwrap_or(0);
        let hi = parts.iter().map(|p| p.0 + p.1).max().unwrap_or(0);
        let (mut min, mut max) = (l.k + lo, l.k + hi);
        for d in 0..3 {
            let span = l.tid[d] * (i64::from(threads[d]) - 1);
            min += span.min(0);
            max += span.max(0);
        }
        (min, max)
    };
    for x in 0..acc.len() {
        for y in x..acc.len() {
            let (la, pa, wa, aa) = &acc[x];
            let (lb, pb, wb, ab) = &acc[y];
            if !(*wa || *wb) || (*aa && *ab) {
                continue;
            }
            if x == y && !*wa {
                continue;
            }
            if la.sym != lb.sym {
                return false;
            }
            let ((a0, a1), (b0, b1)) = (bounds(la, pa), bounds(lb, pb));
            if a1 <= b0 || b1 <= a0 {
                continue;
            }
            // the same per-thread slot: one thread's bytes of both within the spacing
            if la.tid == lb.tid && !la.uniform() {
                let lo = pa.iter().chain(pb.iter()).map(|p| p.0).min().unwrap_or(0);
                let lo = (la.k + lo).min(lb.k + lo);
                let hi = pa
                    .iter()
                    .map(|p| la.k + p.0 + p.1)
                    .chain(pb.iter().map(|p| lb.k + p.0 + p.1))
                    .max()
                    .unwrap_or(0);
                if injective(&la.tid, threads, hi - lo) {
                    continue;
                }
            }
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::injective;

    #[test]
    fn thread_slots() {
        // 64 threads in x, 16 bytes each: apart
        assert!(injective(&[16, 0, 0], [64, 1, 1], 16));
        // 16 bytes each but only 8 apart: overlap
        assert!(!injective(&[8, 0, 0], [64, 1, 1], 16));
        // 8 x 8: x by 4, y by 32 = 8 * 4: every thread its own dword
        assert!(injective(&[4, 32, 0], [8, 8, 1], 4));
        // y by 16 < 8 * 4: rows overlap
        assert!(!injective(&[4, 16, 0], [8, 8, 1], 4));
        // x alone in an 8 x 8 group: threads of one column share it
        assert!(!injective(&[4, 0, 0], [8, 8, 1], 4));
    }
}
