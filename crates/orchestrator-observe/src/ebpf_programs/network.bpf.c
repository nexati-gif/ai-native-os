// eBPF program for network observation

#include <linux/bpf.h>
#include <linux/if_ether.h>
#include <linux/ip.h>
#include <linux/tcp.h>
#include <linux/udp.h>
#include <linux/types.h>
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_tracing.h>
#include <bpf/bpf_core_read.h>

#define MAX_PORTS 65536

// Ring buffer for network events
struct {
    __uint(type, BPF_MAP_TYPE_RINGBUF);
    __uint(max_entries, 1 << 24);
} network_events SEC(".maps");

// Per-CPU network stats
struct {
    __uint(type, BPF_MAP_TYPE_PERCPU_ARRAY);
    __uint(max_entries, 256);
    __type(key, __u32);
    __type(value, struct net_cpu_stats);
} net_cpu_stats_map SEC(".maps");

struct net_cpu_stats {
    __u64 rx_packets;
    __u64 tx_packets;
    __u64 rx_bytes;
    __u64 tx_bytes;
    __u64 rx_dropped;
    __u64 tx_dropped;
    __u64 rx_errors;
    __u64 tx_errors;
    __u64 timestamp;
};

// Connection tracking
struct {
    __uint(type, BPF_MAP_TYPE_LRU_HASH);
    __uint(max_entries, 10000);
    __type(key, struct conn_key);
    __type(value, struct conn_stats);
} conn_tracker SEC(".maps");

struct conn_key {
    __u32 saddr;
    __u32 daddr;
    __u16 sport;
    __u16 dport;
    __u8 protocol;
    __u8 pad[3];
};

struct conn_stats {
    __u64 packets;
    __u64 bytes;
    __u64 retransmits;
    __u64 rtt_sum;
    __u64 rtt_count;
    __u64 start_time;
    __u64 last_seen;
};

// TCP latency histogram
struct {
    __uint(type, BPF_MAP_TYPE_HISTOGRAM);
    __uint(max_entries, 64);
    __type(key, __u32);
    __type(value, __u64);
} tcp_rtt_hist SEC(".maps");

// Network event types
struct net_packet_event {
    __u64 timestamp;
    __u32 cpu;
    __u32 ifindex;
    __u32 len;
    __u32 protocol;
    __u32 saddr;
    __u32 daddr;
    __u16 sport;
    __u16 dport;
    __u8 direction; // 0=rx, 1=tx
    __u8 tcp_flags;
    __u16 pad;
};

struct net_tcp_event {
    __u64 timestamp;
    __u32 cpu;
    __u32 saddr;
    __u32 daddr;
    __u16 sport;
    __u16 dport;
    __u32 seq;
    __u32 ack_seq;
    __u8 flags;
    __u8 state;
    __u16 rtt_us;
    __u32 cwnd;
    __u32 ssthresh;
};

struct net_drop_event {
    __u64 timestamp;
    __u32 cpu;
    __u32 ifindex;
    __u32 reason;
    __u32 len;
    __u32 saddr;
    __u32 daddr;
    __u16 sport;
    __u16 dport;
};

// XDP program for packet observation
SEC("xdp")
int xdp_observe(struct xdp_md *ctx) {
    void *data = (void *)(long)ctx->data;
    void *data_end = (void *)(long)ctx->data_end;
    
    struct ethhdr *eth = data;
    if ((void *)(eth + 1) > data_end) return XDP_PASS;
    
    __u32 protocol = bpf_ntohs(eth->h_proto);
    if (protocol != ETH_P_IP) return XDP_PASS;
    
    struct iphdr *ip = (void *)(eth + 1);
    if ((void *)(ip + 1) > data_end) return XDP_PASS;
    
    __u32 saddr = ip->saddr;
    __u32 daddr = ip->daddr;
    __u8 ip_proto = ip->protocol;
    
    __u16 sport = 0, dport = 0;
    void *transport = (void *)(ip + 1);
    
    if (ip_proto == IPPROTO_TCP) {
        struct tcphdr *tcp = transport;
        if ((void *)(tcp + 1) > data_end) return XDP_PASS;
        sport = bpf_ntohs(tcp->source);
        dport = bpf_ntohs(tcp->dest);
    } else if (ip_proto == IPPROTO_UDP) {
        struct udphdr *udp = transport;
        if ((void *)(udp + 1) > data_end) return XDP_PASS;
        sport = bpf_ntohs(udp->source);
        dport = bpf_ntohs(udp->dest);
    }
    
    // Record packet event
    struct net_packet_event *event;
    event = bpf_ringbuf_reserve(&network_events, sizeof(*event), 0);
    if (event) {
        event->timestamp = bpf_ktime_get_ns();
        event->cpu = bpf_get_smp_processor_id();
        event->ifindex = ctx->ingress_ifindex;
        event->len = ctx->data_end - ctx->data;
        event->protocol = ip_proto;
        event->saddr = saddr;
        event->daddr = daddr;
        event->sport = sport;
        event->dport = dport;
        event->direction = 0; // RX
        event->tcp_flags = ip_proto == IPPROTO_TCP ? ((struct tcphdr *)transport)->syn | ((struct tcphdr *)transport)->ack << 1 : 0;
        
        bpf_ringbuf_submit(event, 0);
    }
    
    // Update per-CPU stats
    __u32 cpu = bpf_get_smp_processor_id();
    struct net_cpu_stats *stats = bpf_map_lookup_elem(&net_cpu_stats_map, &cpu);
    if (stats) {
        __sync_fetch_and_add(&stats->rx_packets, 1);
        __sync_fetch_and_add(&stats->rx_bytes, ctx->data_end - ctx->data);
        stats->timestamp = bpf_ktime_get_ns();
    }
    
    // Track connection
    struct conn_key key = { .saddr = saddr, .daddr = daddr, .sport = sport, .dport = dport, .protocol = ip_proto };
    struct conn_stats *cstats = bpf_map_lookup_elem(&conn_tracker, &key);
    if (!cstats) {
        struct conn_stats new_stats = {
            .packets = 1,
            .bytes = ctx->data_end - ctx->data,
            .start_time = bpf_ktime_get_ns(),
            .last_seen = bpf_ktime_get_ns(),
        };
        bpf_map_update_elem(&conn_tracker, &key, &new_stats, BPF_ANY);
    } else {
        __sync_fetch_and_add(&cstats->packets, 1);
        __sync_fetch_and_add(&cstats->bytes, ctx->data_end - ctx->data);
        cstats->last_seen = bpf_ktime_get_ns();
    }
    
    return XDP_PASS;
}

// Tracepoint: net:net_dev_xmit (TX packets)
SEC("tp_btf/net_dev_xmit")
int BPF_PROG(net_dev_xmit, struct sk_buff *skb, struct net_device *dev, int (*xmit)(struct sk_buff *)) {
    // Similar to XDP but for TX
    return 0;
}

// Tracepoint: net:net_dev_queue (TX queue)
SEC("tp_btf/net_dev_queue")
int BPF_PROG(net_dev_queue, struct sk_buff *skb, struct net_device *dev, struct netdev_queue *txq) {
    return 0;
}

// Tracepoint: net:netif_receive_skb (RX)
SEC("tp_btf/netif_receive_skb")
int BPF_PROG(netif_receive_skb, struct sk_buff *skb) {
    return 0;
}

// Tracepoint: tcp:tcp_retransmit_skb
SEC("tp_btf/tcp_retransmit_skb")
int BPF_PROG(tcp_retransmit_skb, struct sock *sk, struct sk_buff *skb) {
    struct conn_key key = {0};
    struct tcp_sock *tp = (struct tcp_sock *)sk;
    
    bpf_probe_read_kernel(&key.saddr, sizeof(key.saddr), &sk->__sk_common.skc_rcv_saddr);
    bpf_probe_read_kernel(&key.daddr, sizeof(key.daddr), &sk->__sk_common.skc_daddr);
    bpf_probe_read_kernel(&key.sport, sizeof(key.sport), &sk->__sk_common.skc_num);
    bpf_probe_read_kernel(&key.dport, sizeof(key.dport), &sk->__sk_common.skc_dport);
    key.protocol = IPPROTO_TCP;
    key.dport = bpf_ntohs(key.dport);
    
    struct conn_stats *cstats = bpf_map_lookup_elem(&conn_tracker, &key);
    if (cstats) {
        __sync_fetch_and_add(&cstats->retransmits, 1);
    }
    
    return 0;
}

// Tracepoint: tcp:tcp_rcv_established (for RTT)
SEC("tp_btf/tcp_rcv_established")
int BPF_PROG(tcp_rcv_established, struct sock *sk, struct sk_buff *skb) {
    struct tcp_sock *tp = (struct tcp_sock *)sk;
    
    // Calculate RTT from srtt
    __u32 srtt = tp->srtt_us >> 3; // srtt is in us * 8
    if (srtt > 0) {
        bpf_map_update_elem(&tcp_rtt_hist, &srtt, &( __u64 ){ 1 }, BPF_ADD);
    }
    
    return 0;
}

// Tracepoint: sock:inet_sock_set_state (connection state changes)
SEC("tp_btf/inet_sock_set_state")
int BPF_PROG(inet_sock_set_state, struct sock *sk, int oldstate, int newstate) {
    if (newstate == TCP_CLOSE || newstate == TCP_TIME_WAIT) {
        // Connection closing, could emit event
    }
    return 0;
}

// Kprobe: kfree_skb (drop tracking)
SEC("kprobe/kfree_skb")
int BPF_KPROBE(kfree_skb, struct sk_buff *skb, void *location) {
    __u32 cpu = bpf_get_smp_processor_id();
    struct net_cpu_stats *stats = bpf_map_lookup_elem(&net_cpu_stats_map, &cpu);
    if (stats) {
        __sync_fetch_and_add(&stats->rx_dropped, 1);
    }
    return 0;
}

char LICENSE[] SEC("license") = "GPL";