struct Params {
    n: u32,
    grid: u32,
};

@group(0) @binding(0) var<storage, read> a_pos: array<u32>;
@group(0) @binding(1) var<storage, read> a_neg: array<u32>;
@group(0) @binding(2) var<storage, read> b_pos: array<u32>;
@group(0) @binding(3) var<storage, read> b_neg: array<u32>;
@group(0) @binding(4) var<storage, read_write> out: array<atomic<i32>>;
@group(0) @binding(5) var<uniform> params: Params;

var<workgroup> scratch: array<i32, 256>;

// 优化版：两组 POPC（pp+nn / pn+np 各合一次）+ 线程内多字网格步进 +
// workgroup 归约后每个 workgroup 只做一次 atomic。
@compute @workgroup_size(256)
fn main(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) lid: u32,
) {
    var sum = 0;
    let stride = 256u * params.grid;
    var i = gid.x;
    loop {
        if (i >= params.n) { break; }
        let ap = a_pos[i];
        let an = a_neg[i];
        let bp = b_pos[i];
        let bn = b_neg[i];
        let matches = countOneBits((ap & bp) | (an & bn));
        let mismatches = countOneBits((ap & bn) | (an & bp));
        sum = sum + i32(matches) - i32(mismatches);
        i = i + stride;
    }
    scratch[lid] = sum;
    workgroupBarrier();
    var offset = 128u;
    loop {
        if (offset == 0u) { break; }
        if (lid < offset) {
            scratch[lid] = scratch[lid] + scratch[lid + offset];
        }
        workgroupBarrier();
        offset = offset >> 1u;
    }
    if (lid == 0u) {
        atomicAdd(&out[0], scratch[0]);
    }
}

// 对照版：四组 POPC + 每个字一次 atomic（即本 PR 之前的写法）。
@compute @workgroup_size(256)
fn main_naive(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= arrayLength(&a_pos)) {
        return;
    }
    let pp = i32(countOneBits(a_pos[i] & b_pos[i]));
    let pn = i32(countOneBits(a_pos[i] & b_neg[i]));
    let np = i32(countOneBits(a_neg[i] & b_pos[i]));
    let nn = i32(countOneBits(a_neg[i] & b_neg[i]));
    atomicAdd(&out[0], pp - pn - np + nn);
}

// 只合并 POPC，不改 atomic 粒度（用来隔离「合并 POPC」这一项）。
@compute @workgroup_size(256)
fn main_popc2(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= arrayLength(&a_pos)) {
        return;
    }
    let matches = countOneBits((a_pos[i] & b_pos[i]) | (a_neg[i] & b_neg[i]));
    let mismatches = countOneBits((a_pos[i] & b_neg[i]) | (a_neg[i] & b_pos[i]));
    atomicAdd(&out[0], i32(matches) - i32(mismatches));
}
