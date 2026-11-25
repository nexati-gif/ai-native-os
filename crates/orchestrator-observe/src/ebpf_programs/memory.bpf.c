// eBPF program for memory observation

#include <linux/bpf.h>
#include <linux/mm.h>
#include <linux/swap.h>
#include <linux/types.h>
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_tracing.h>
#include <bpf/bpf_core_read.h>

#define MAX_NODES 8

// Ring buffer for memory events
struct {
    __uint(type, BPF_MAP_TYPE_RINGBUF);
    __uint(max_entries, 1 << 24);
} memory_events SEC(".maps");

// Per-NUMA node stats
struct {
    __uint(type, BPF_MAP_TYPE_ARRAY);
    __uint(max_entries, MAX_NODES);
    __type(key, __u32);
    __type(value, struct numa_stats);
} numa_stats_map SEC(".maps");

struct numa_stats {
    __u64 total_pages;
    __u64 free_pages;
    __u64 active_pages;
    __u64 inactive_pages;
    __u64 file_pages;
    __u64 anon_pages;
    __u64 slab_pages;
    __u64 kernel_pages;
    __u64 swap_pages;
    __u64 reclaim_events;
    __u64 reclaim_pages;
    __u64 compact_events;
    __u64 compact_pages;
    __u64 oom_events;
    __u64 timestamp;
};

// Page fault latency histogram
struct {
    __uint(type, BPF_MAP_TYPE_HISTOGRAM);
    __uint(max_entries, 64);
    __type(key, __u32);
    __type(value, __u64);
} page_fault_latency_hist SEC(".maps");

// Swap latency histogram
struct {
    __uint(type, BPF_MAP_TYPE_HISTOGRAM);
    __uint(max_entries, 64);
    __type(key, __u32);
    __type(value, __u64);
} swap_latency_hist SEC(".maps");

// In-flight page fault tracking
struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, 10000);
    __type(key, struct pt_regs *);
    __type(value, __u64);
} fault_start_time SEC(".maps");

// Memory event structures
struct mem_page_fault_event {
    __u64 timestamp;
    __u32 cpu;
    __u32 pid;
    __u32 tid;
    __u64 address;
    __u32 flags; // VM_FAULT_*
    __u32 error_code;
    __u8 type; // 0=major, 1=minor, 2=swap
    __u8 pad[3];
    __u64 latency_ns;
};

struct mem_reclaim_event {
    __u64 timestamp;
    __u32 cpu;
    __u32 nid;
    __u64 pages_scanned;
    __u64 pages_reclaimed;
    __u64 pages_dirty;
    __u64 pages_writeback;
    __u8 priority;
    __u8 may_swap;
    __u16 pad;
    __u64 latency_ns;
};

struct mem_compact_event {
    __u64 timestamp;
    __u32 cpu;
    __u32 nid;
    __u64 pages_moved;
    __u64 pages_failed;
    __u8 order;
    __u8 sync;
    __u16 pad;
    __u64 latency_ns;
};

struct mem_oom_event {
    __u64 timestamp;
    __u32 cpu;
    __u32 pid;
    __u32 tid;
    __u64 mem_cgroup_id;
    __u64 total_pages;
    __u64 free_pages;
    char comm[16];
};

// Tracepoint: mm:page_fault_user / mm:page_fault_kernel
SEC("tp_btf/page_fault_user")
int BPF_PROG(page_fault_user, struct pt_regs *regs, unsigned long address, unsigned long error_code) {
    __u64 start = bpf_ktime_get_ns();
    bpf_map_update_elem(&fault_start_time, &regs, &start, BPF_ANY);
    return 0;
}

SEC("tp_btf/page_fault_kernel")
int BPF_PROG(page_fault_kernel, struct pt_regs *regs, unsigned long address, unsigned long error_code) {
    __u64 start = bpf_ktime_get_ns();
    bpf_map_update_elem(&fault_start_time, &regs, &start, BPF_ANY);
    return 0;
}

// Tracepoint: mm:page_fault_enter (alternative)
SEC("tp_btf/page_fault_enter")
int BPF_PROG(page_fault_enter, struct pt_regs *regs, unsigned long address) {
    __u64 start = bpf_ktime_get_ns();
    bpf_map_update_elem(&fault_start_time, &regs, &start, BPF_ANY);
    return 0;
}

// Tracepoint: mm:page_fault_exit
SEC("tp_btf/page_fault_exit")
int BPF_PROG(page_fault_exit, struct pt_regs *regs, unsigned long address, int error_code) {
    __u64 *start = bpf_map_lookup_elem(&fault_start_time, &regs);
    if (!start) return 0;
    
    __u64 latency = bpf_ktime_get_ns() - *start;
    bpf_map_delete_elem(&fault_start_time, &regs);
    
    // Determine fault type from error_code
    bool major = (error_code & 0x8) != 0; // VM_FAULT_MAJOR
    bool minor = !major && (error_code & 0x1) == 0; // Not present, not major
    bool swap = (error_code & 0x10) != 0; // Swap fault
    
    __u8 type = major ? 0 : (swap ? 2 : 1);
    
    // Update histogram
    __u32 latency_us = latency / 1000;
    if (type == 2) {
        bpf_map_update_elem(&swap_latency_hist, &latency_us, &( __u64 ){ 1 }, BPF_ADD);
    } else {
        bpf_map_update_elem(&page_fault_latency_hist, &latency_us, &( __u64 ){ 1 }, BPF_ADD);
    }
    
    // Emit event
    struct mem_page_fault_event *event;
    event = bpf_ringbuf_reserve(&memory_events, sizeof(*event), 0);
    if (event) {
        event->timestamp = bpf_ktime_get_ns();
        event->cpu = bpf_get_smp_processor_id();
        
        struct task_struct *task = (struct task_struct *)bpf_get_current_task();
        event->pid = task->tgid;
        event->tid = task->pid;
        
        event->address = address;
        event->error_code = error_code;
        event->type = type;
        event->latency_ns = latency;
        
        bpf_ringbuf_submit(event, 0);
    }
    
    return 0;
}

// Tracepoint: mm:mm_page_alloc (page allocation)
SEC("tp_btf/mm_page_alloc")
int BPF_PROG(mm_page_alloc, struct page *page, unsigned int order, gfp_t gfp_flags, int migratetype) {
    // Track allocations
    return 0;
}

// Tracepoint: mm:mm_page_free (page free)
SEC("tp_btf/mm_page_free")
int BPF_PROG(mm_page_free, struct page *page, unsigned int order) {
    // Track frees
    return 0;
}

// Tracepoint: mm:mm_page (page state changes)
SEC("tp_btf/mm_page")
int BPF_PROG(mm_page, struct page *page, unsigned long pfn, int order, int migratetype, int percpu_refill, int gfp_flags) {
    return 0;
}

// Tracepoint: vmscan:mm_vmscan_kswapd_sleep
SEC("tp_btf/mm_vmscan_kswapd_sleep")
int BPF_PROG(mm_vmscan_kswapd_sleep, int nid) {
    return 0;
}

// Tracepoint: vmscan:mm_vmscan_kswapd_wake
SEC("tp_btf/mm_vmscan_kswapd_wake")
int BPF_PROG(mm_vmscan_kswapd_wake, int nid, int order) {
    return 0;
}

// Tracepoint: vmscan:mm_vmscan_direct_reclaim_begin
SEC("tp_btf/mm_vmscan_direct_reclaim_begin")
int BPF_PROG(mm_vmscan_direct_reclaim_begin, int nid, int order) {
    // Could track start time for reclaim latency
    return 0;
}

// Tracepoint: vmscan:mm_vmscan_direct_reclaim_end
SEC("tp_btf/mm_vmscan_direct_reclaim_end")
int BPF_PROG(mm_vmscan_direct_reclaim_end, int nid, int order, unsigned long reclaimed) {
    return 0;
}

// Tracepoint: vmscan:mm_vmscan_reclaim_pages
SEC("tp_btf/mm_vmscan_reclaim_pages")
int BPF_PROG(mm_vmscan_reclaim_pages, int nid, unsigned long pages_scanned, unsigned long pages_reclaimed, unsigned long pages_dirty, unsigned long pages_writeback, int priority, int may_swap) {
    __u64 start = bpf_ktime_get_ns(); // Simplified - would need to track start
    
    struct mem_reclaim_event *event;
    event = bpf_ringbuf_reserve(&memory_events, sizeof(*event), 0);
    if (event) {
        event->timestamp = start;
        event->cpu = bpf_get_smp_processor_id();
        event->nid = nid;
        event->pages_scanned = pages_scanned;
        event->pages_reclaimed = pages_reclaimed;
        event->pages_dirty = pages_dirty;
        event->pages_writeback = pages_writeback;
        event->priority = priority;
        event->may_swap = may_swap;
        event->latency_ns = 0; // Would calculate from start
        
        bpf_ringbuf_submit(event, 0);
    }
    
    // Update NUMA stats
    if (nid < MAX_NODES) {
        struct numa_stats *stats = bpf_map_lookup_elem(&numa_stats_map, &nid);
        if (stats) {
            __sync_fetch_and_add(&stats->reclaim_events, 1);
            __sync_fetch_and_add(&stats->reclaim_pages, pages_reclaimed);
            stats->timestamp = start;
        }
    }
    
    return 0;
}

// Tracepoint: compaction:mm_compaction_begin
SEC("tp_btf/mm_compaction_begin")
int BPF_PROG(mm_compaction_begin, int nid, int order, int sync) {
    return 0;
}

// Tracepoint: compaction:mm_compaction_end
SEC("tp_btf/mm_compaction_end")
int BPF_PROG(mm_compaction_end, int nid, int order, int sync, unsigned long pages_moved, unsigned long pages_failed) {
    struct mem_compact_event *event;
    event = bpf_ringbuf_reserve(&memory_events, sizeof(*event), 0);
    if (event) {
        event->timestamp = bpf_ktime_get_ns();
        event->cpu = bpf_get_smp_processor_id();
        event->nid = nid;
        event->pages_moved = pages_moved;
        event->pages_failed = pages_failed;
        event->order = order;
        event->sync = sync;
        event->latency_ns = 0;
        
        bpf_ringbuf_submit(event, 0);
    }
    
    if (nid < MAX_NODES) {
        struct numa_stats *stats = bpf_map_lookup_elem(&numa_stats_map, &nid);
        if (stats) {
            __sync_fetch_and_add(&stats->compact_events, 1);
            __sync_fetch_and_add(&stats->compact_pages, pages_moved);
        }
    }
    
    return 0;
}

// Tracepoint: oom:mm_oom_kill
SEC("tp_btf/mm_oom_kill")
int BPF_PROG(mm_oom_kill, struct task_struct *task, int pid, unsigned long pages, int order, int oom_score_adj) {
    struct mem_oom_event *event;
    event = bpf_ringbuf_reserve(&memory_events, sizeof(*event), 0);
    if (event) {
        event->timestamp = bpf_ktime_get_ns();
        event->cpu = bpf_get_smp_processor_id();
        event->pid = task->tgid;
        event->tid = task->pid;
        event->mem_cgroup_id = 0; // Would need cgroup tracking
        event->total_pages = totalram_pages();
        event->free_pages = nr_free_pages();
        
        bpf_probe_read_kernel_str(event->comm, sizeof(event->comm), task->comm);
        
        bpf_ringbuf_submit(event, 0);
    }
    
    if (0 < MAX_NODES) { // Global OOM counter
        struct numa_stats *stats = bpf_map_lookup_elem(&numa_stats_map, &( __u32 ){ 0 });
        if (stats) {
            __sync_fetch_and_add(&stats->oom_events, 1);
        }
    }
    
    return 0;
}

// Kprobe: swap_readpage / swap_writepage
SEC("kprobe/swap_readpage")
int BPF_KPROBE(swap_readpage, struct bio *bio) {
    __u64 start = bpf_ktime_get_ns();
    bpf_map_update_elem(&fault_start_time, &bio, &start, BPF_ANY);
    return 0;
}

SEC("kretprobe/swap_readpage")
int BPF_KRETPROBE(swap_readpage_ret, int ret) {
    struct bio *bio = (struct bio *)PT_REGS_PARM1(ctx);
    __u64 *start = bpf_map_lookup_elem(&fault_start_time, &bio);
    if (!start) return 0;
    
    __u64 latency = bpf_ktime_get_ns() - *start;
    bpf_map_delete_elem(&fault_start_time, &bio);
    
    __u32 latency_us = latency / 1000;
    bpf_map_update_elem(&swap_latency_hist, &latency_us, &( __u64 ){ 1 }, BPF_ADD);
    
    return 0;
}

// Kprobe: shrink_slab
SEC("kprobe/shrink_slab")
int BPF_KPROBE(shrink_slab, struct shrinker *shrinker, struct shrink_control *sc) {
    return 0;
}

// Tracepoint: kmem:mm_slab_alloc / mm_slab_free
SEC("tp_btf/mm_slab_alloc")
int BPF_PROG(mm_slab_alloc, void *ptr, size_t size, size_t align, gfp_t gfp_flags, int nid) {
    if (nid < MAX_NODES) {
        struct numa_stats *stats = bpf_map_lookup_elem(&numa_stats_map, &nid);
        if (stats) {
            __sync_fetch_and_add(&stats->slab_pages, size / PAGE_SIZE);
        }
    }
    return 0;
}

char LICENSE[] SEC("license") = "GPL";