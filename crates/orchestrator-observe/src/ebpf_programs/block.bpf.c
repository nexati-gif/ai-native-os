// eBPF program for block I/O observation

#include <linux/bpf.h>
#include <linux/blkdev.h>
#include <linux/types.h>
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_tracing.h>
#include <bpf/bpf_core_read.h>

#define MAX_DISKS 64

// Ring buffer for block events
struct {
    __uint(type, BPF_MAP_TYPE_RINGBUF);
    __uint(max_entries, 1 << 24);
} block_events SEC(".maps");

// Per-disk stats
struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, MAX_DISKS);
    __type(key, dev_t);
    __type(value, struct disk_stats);
} disk_stats_map SEC(".maps");

struct disk_stats {
    __u64 reads;
    __u64 writes;
    __u64 read_bytes;
    __u64 write_bytes;
    __u64 read_latency_sum;
    __u64 write_latency_sum;
    __u64 flushes;
    __u64 discards;
    __u64 errors;
    __u64 in_flight;
    __u64 timestamp;
};

// I/O latency histograms
struct {
    __uint(type, BPF_MAP_TYPE_HISTOGRAM);
    __uint(max_entries, 64);
    __type(key, __u32);
    __type(value, __u64);
} read_latency_hist SEC(".maps");

struct {
    __uint(type, BPF_MAP_TYPE_HISTOGRAM);
    __uint(max_entries, 64);
    __type(key, __u32);
    __type(value, __u64);
} write_latency_hist SEC(".maps");

// In-flight I/O tracking
struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, 10000);
    __type(key, struct request *);
    __type(value, __u64);
} io_start_time SEC(".maps");

// Block event structures
struct block_rq_event {
    __u64 timestamp;
    __u32 cpu;
    dev_t dev;
    __u32 sector;
    __u32 nr_sectors;
    __u8 rw; // 0=read, 1=write, 2=flush, 3=discard
    __u8 flags;
    __u16 pad;
    __u64 latency_ns;
};

struct block_rq_complete_event {
    __u64 timestamp;
    __u32 cpu;
    dev_t dev;
    __u32 sector;
    __u32 nr_sectors;
    __u8 rw;
    __u8 error;
    __u16 pad;
    __u64 latency_ns;
};

// Tracepoint: block:block_rq_issue
SEC("tp_btf/block_rq_issue")
int BPF_PROG(block_rq_issue, struct request *rq) {
    dev_t dev = rq->rq_disk ? rq->rq_disk->devt : 0;
    if (!dev) return 0;
    
    // Record start time for latency calculation
    __u64 start = bpf_ktime_get_ns();
    bpf_map_update_elem(&io_start_time, &rq, &start, BPF_ANY);
    
    // Increment in-flight counter
    struct disk_stats *stats = bpf_map_lookup_elem(&disk_stats_map, &dev);
    if (stats) {
        __sync_fetch_and_add(&stats->in_flight, 1);
        stats->timestamp = start;
    }
    
    return 0;
}

// Tracepoint: block:block_rq_complete
SEC("tp_btf/block_rq_complete")
int BPF_PROG(block_rq_complete, struct request *rq, blk_status_t error, unsigned int nr_bytes) {
    dev_t dev = rq->rq_disk ? rq->rq_disk->devt : 0;
    if (!dev) return 0;
    
    __u64 *start = bpf_map_lookup_elem(&io_start_time, &rq);
    __u64 latency = 0;
    if (start) {
        latency = bpf_ktime_get_ns() - *start;
        bpf_map_delete_elem(&io_start_time, &rq);
    }
    
    struct disk_stats *stats = bpf_map_lookup_elem(&disk_stats_map, &dev);
    if (!stats) {
        struct disk_stats new_stats = {0};
        bpf_map_update_elem(&disk_stats_map, &dev, &new_stats, BPF_ANY);
        stats = bpf_map_lookup_elem(&disk_stats_map, &dev);
    }
    
    if (stats) {
        __sync_fetch_and_add(&stats->in_flight, -1);
        
        bool is_write = (rq->cmd_flags & REQ_WRITE) != 0;
        bool is_flush = (rq->cmd_flags & REQ_FLUSH) != 0;
        bool is_discard = (rq->cmd_flags & REQ_DISCARD) != 0;
        
        if (is_write) {
            __sync_fetch_and_add(&stats->writes, 1);
            __sync_fetch_and_add(&stats->write_bytes, nr_bytes);
            __sync_fetch_and_add(&stats->write_latency_sum, latency);
            bpf_map_update_elem(&write_latency_hist, &( __u32 ){ latency / 1000 }, &( __u64 ){ 1 }, BPF_ADD);
        } else if (is_flush) {
            __sync_fetch_and_add(&stats->flushes, 1);
        } else if (is_discard) {
            __sync_fetch_and_add(&stats->discards, 1);
        } else {
            __sync_fetch_and_add(&stats->reads, 1);
            __sync_fetch_and_add(&stats->read_bytes, nr_bytes);
            __sync_fetch_and_add(&stats->read_latency_sum, latency);
            bpf_map_update_elem(&read_latency_hist, &( __u32 ){ latency / 1000 }, &( __u64 ){ 1 }, BPF_ADD);
        }
        
        if (error) {
            __sync_fetch_and_add(&stats->errors, 1);
        }
        
        stats->timestamp = bpf_ktime_get_ns();
    }
    
    // Emit completion event
    struct block_rq_complete_event *event;
    event = bpf_ringbuf_reserve(&block_events, sizeof(*event), 0);
    if (event) {
        event->timestamp = bpf_ktime_get_ns();
        event->cpu = bpf_get_smp_processor_id();
        event->dev = dev;
        event->sector = rq->__sector;
        event->nr_sectors = blk_rq_sectors(rq);
        event->rw = (rq->cmd_flags & REQ_WRITE) ? 1 : 0;
        if (rq->cmd_flags & REQ_FLUSH) event->rw = 2;
        if (rq->cmd_flags & REQ_DISCARD) event->rw = 3;
        event->error = error;
        event->latency_ns = latency;
        
        bpf_ringbuf_submit(event, 0);
    }
    
    return 0;
}

// Tracepoint: block:block_rq_requeue
SEC("tp_btf/block_rq_requeue")
int BPF_PROG(block_rq_requeue, struct request *rq) {
    // Requeued request - could track retries
    return 0;
}

// Tracepoint: block:block_bio_complete
SEC("tp_btf/block_bio_complete")
int BPF_PROG(block_bio_complete, struct bio *bio, blk_status_t error) {
    // Bio-level completion
    return 0;
}

// Tracepoint: block:block_split
SEC("tp_btf/block_split")
int BPF_PROG(block_split, struct bio *bio, unsigned int sectors) {
    // Bio splitting
    return 0;
}

// Tracepoint: block:block_unplug
SEC("tp_btf/block_unplug")
int BPF_PROG(block_unplug, struct request_queue *q, unsigned int depth, bool from_schedule) {
    // Request queue unplugged
    return 0;
}

// Kprobe: blk_account_io_done
SEC("kprobe/blk_account_io_done")
int BPF_KPROBE(blk_account_io_done, struct request *rq) {
    // Accounting
    return 0;
}

// Tracepoint: block:block_rq_abort
SEC("tp_btf/block_rq_abort")
int BPF_PROG(block_rq_abort, struct request *rq) {
    // Request aborted
    return 0;
}

char LICENSE[] SEC("license") = "GPL";