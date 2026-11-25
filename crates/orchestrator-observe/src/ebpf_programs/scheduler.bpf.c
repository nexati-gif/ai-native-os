// eBPF program for scheduler observation
// Compiles with: clang -target bpf -O2 -c scheduler.bpf.c -o scheduler.bpf.o

#include <linux/bpf.h>
#include <linux/sched.h>
#include <linux/types.h>
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_tracing.h>
#include <bpf/bpf_core_read.h>

#define MAX_CPUS 256

// Ring buffer for events
struct {
    __uint(type, BPF_MAP_TYPE_RINGBUF);
    __uint(max_entries, 1 << 24); // 16MB
} scheduler_events SEC(".maps");

// Per-CPU runqueue stats
struct {
    __uint(type, BPF_MAP_TYPE_PERCPU_ARRAY);
    __uint(max_entries, MAX_CPUS);
    __type(key, __u32);
    __type(value, struct runqueue_stats);
} runqueue_stats_map SEC(".maps");

struct runqueue_stats {
    __u64 nr_running;
    __u64 nr_runnable;
    __u64 nr_uninterruptible;
    __u64 nr_iowait;
    __u64 load_avg;
    __u64 util_avg;
    __u64 timestamp;
};

// Scheduler latency histogram
struct {
    __uint(type, BPF_MAP_TYPE_HISTOGRAM);
    __uint(max_entries, 64);
    __type(key, __u32);
    __type(value, __u64);
} sched_latency_hist SEC(".maps");

// Context switch events
struct sched_switch_event {
    __u64 timestamp;
    __u32 cpu;
    __u32 prev_pid;
    __u32 next_pid;
    __u32 prev_prio;
    __u32 next_prio;
    __u64 prev_runtime;
    __u64 next_runtime;
    char prev_comm[16];
    char next_comm[16];
};

// Wakeup events
struct sched_wakeup_event {
    __u64 timestamp;
    __u32 cpu;
    __u32 pid;
    __u32 target_cpu;
    __u32 prio;
    char comm[16];
};

// Load balance events
struct sched_lb_event {
    __u64 timestamp;
    __u32 cpu;
    __u32 src_cpu;
    __u32 dst_cpu;
    __u32 nr_tasks_moved;
    __s32 imbalance;
};

// Tracepoint: sched:sched_switch
SEC("tp_btf/sched_switch")
int BPF_PROG(sched_switch, struct task_struct *prev, struct task_struct *next) {
    struct sched_switch_event *event;
    event = bpf_ringbuf_reserve(&scheduler_events, sizeof(*event), 0);
    if (!event) return 0;
    
    event->timestamp = bpf_ktime_get_ns();
    event->cpu = bpf_get_smp_processor_id();
    event->prev_pid = prev->pid;
    event->next_pid = next->pid;
    event->prev_prio = prev->prio;
    event->next_prio = next->prio;
    event->prev_runtime = prev->se.sum_exec_runtime;
    event->next_runtime = next->se.sum_exec_runtime;
    
    bpf_probe_read_kernel_str(event->prev_comm, sizeof(event->prev_comm), prev->comm);
    bpf_probe_read_kernel_str(event->next_comm, sizeof(event->next_comm), next->comm);
    
    bpf_ringbuf_submit(event, 0);
    return 0;
}

// Tracepoint: sched:sched_wakeup
SEC("tp_btf/sched_wakeup")
int BPF_PROG(sched_wakeup, struct task_struct *p) {
    struct sched_wakeup_event *event;
    event = bpf_ringbuf_reserve(&scheduler_events, sizeof(*event), 0);
    if (!event) return 0;
    
    event->timestamp = bpf_ktime_get_ns();
    event->cpu = bpf_get_smp_processor_id();
    event->pid = p->pid;
    event->target_cpu = task_cpu(p);
    event->prio = p->prio;
    
    bpf_probe_read_kernel_str(event->comm, sizeof(event->comm), p->comm);
    
    bpf_ringbuf_submit(event, 0);
    return 0;
}

// Tracepoint: sched:sched_wakeup_new
SEC("tp_btf/sched_wakeup_new")
int BPF_PROG(sched_wakeup_new, struct task_struct *p) {
    // Same as sched_wakeup for new tasks
    return sched_wakeup(ctx, p);
}

// Tracepoint: sched:sched_process_fork
SEC("tp_btf/sched_process_fork")
int BPF_PROG(sched_process_fork, struct task_struct *parent, struct task_struct *child) {
    // Track fork rates
    return 0;
}

// Tracepoint: sched:sched_process_exit
SEC("tp_btf/sched_process_exit")
int BPF_PROG(sched_process_exit, struct task_struct *p) {
    // Track exit rates
    return 0;
}

// Kprobe: finish_task_switch (for runqueue stats)
SEC("kprobe/finish_task_switch")
int BPF_KPROBE(finish_task_switch, struct task_struct *prev) {
    __u32 cpu = bpf_get_smp_processor_id();
    struct runqueue_stats *stats = bpf_map_lookup_elem(&runqueue_stats_map, &cpu);
    if (!stats) return 0;
    
    struct rq *rq = cpu_rq(cpu);
    stats->nr_running = rq->nr_running;
    stats->nr_uninterruptible = rq->nr_uninterruptible;
    stats->nr_iowait = rq->nr_iowait;
    stats->load_avg = rq->avg_load;
    stats->util_avg = rq->avg_util;
    stats->timestamp = bpf_ktime_get_ns();
    
    return 0;
}

// Tracepoint: sched:sched_load_balance (for load balancing observation)
SEC("tp_btf/sched_load_balance")
int BPF_PROG(sched_load_balance, int cpu, struct sched_domain *sd, int idle, int *continue_balancing) {
    struct sched_lb_event *event;
    event = bpf_ringbuf_reserve(&scheduler_events, sizeof(*event), 0);
    if (!event) return 0;
    
    event->timestamp = bpf_ktime_get_ns();
    event->cpu = cpu;
    event->src_cpu = cpu;
    event->dst_cpu = 0; // Would need more context
    event->nr_tasks_moved = 0;
    event->imbalance = 0;
    
    bpf_ringbuf_submit(event, 0);
    return 0;
}

char LICENSE[] SEC("license") = "GPL";