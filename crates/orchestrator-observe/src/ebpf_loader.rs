//! eBPF program loader using aya

use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use aya::{Ebpf, EbpfLoader, maps::RingBuf, programs::{TracePoint, Xdp, KProbe, KRetProbe}};
use aya::maps::MapData;
use orchestrator_common::{Error, Result, EventBus, Event, MetricsRecorder};
use orchestrator_common::types::{MetricSample, AnomalySeverity, DeviceId};
use chrono::{DateTime, Utc};

/// eBPF program manager
pub struct EbpfManager {
    ebpf: Arc<Mutex<Option<Ebpf>>>,
    event_bus: EventBus,
    metrics: MetricsRecorder,
    device_id: DeviceId,
    ring_buffers: Arc<Mutex<HashMap<String, RingBuf>>>,
}

impl EbpfManager {
    pub async fn new(event_bus: EventBus) -> Result<Self> {
        let device_id = DeviceId::new(&hostname::get()?.to_string_lossy());
        
        Ok(Self {
            ebpf: Arc::new(Mutex::new(None)),
            event_bus,
            metrics: MetricsRecorder::new("ebpf"),
            device_id,
            ring_buffers: Arc::new(Mutex::new(HashMap::new())),
        })
    }
    
    /// Load all eBPF programs
    pub async fn load_programs(&self) -> Result<()> {
        let mut ebpf_guard = self.ebpf.lock().await;
        
        if ebpf_guard.is_some() {
            return Ok(());
        }
        
        // Load eBPF object files
        let mut ebpf = EbpfLoader::new()
            .set_btf_enabled(true)
            .load_file(self.get_ebpf_path("scheduler")?)?;
        
        // Load additional programs
        for name in ["network", "block", "memory"] {
            let path = self.get_ebpf_path(name)?;
            if path.exists() {
                ebpf.load_file(&path)?;
            }
        }
        
        // Attach tracepoints
        self.attach_tracepoints(&mut ebpf).await?;
        
        // Setup ring buffers
        self.setup_ring_buffers(&ebpf).await?;
        
        *ebpf_guard = Some(ebpf);
        
        self.metrics.counter("programs_loaded", 1);
        Ok(())
    }
    
    fn get_ebpf_path(&self, name: &str) -> Result<PathBuf> {
        let out_dir = env!("OUT_DIR");
        Ok(PathBuf::from(out_dir).join(format!("{}.bpf.o", name)))
    }
    
    async fn attach_tracepoints(&self, ebpf: &mut Ebpf) -> Result<()> {
        // Scheduler tracepoints (format: "category:name")
        self.attach_tp(ebpf, "sched_switch", "sched:sched_switch")?;
        self.attach_tp(ebpf, "sched_wakeup", "sched:sched_wakeup")?;
        self.attach_tp(ebpf, "sched_wakeup_new", "sched:sched_wakeup_new")?;
        self.attach_tp(ebpf, "sched_process_fork", "sched:sched_process_fork")?;
        self.attach_tp(ebpf, "sched_process_exit", "sched:sched_process_exit")?;
        self.attach_tp(ebpf, "sched_load_balance", "sched:sched_load_balance")?;
        
        // Kprobes for scheduler
        self.attach_kprobe(ebpf, "finish_task_switch", "finish_task_switch")?;
        
        // Network tracepoints
        self.attach_tp(ebpf, "net_dev_xmit", "net:net_dev_xmit")?;
        self.attach_tp(ebpf, "net_dev_queue", "net:net_dev_queue")?;
        self.attach_tp(ebpf, "netif_receive_skb", "net:netif_receive_skb")?;
        self.attach_tp(ebpf, "tcp_retransmit_skb", "tcp:tcp_retransmit_skb")?;
        self.attach_tp(ebpf, "tcp_rcv_established", "tcp:tcp_rcv_established")?;
        self.attach_tp(ebpf, "inet_sock_set_state", "sock:inet_sock_set_state")?;
        
        // Kprobes for network
        self.attach_kprobe(ebpf, "kfree_skb", "kfree_skb")?;
        
        // Block tracepoints
        self.attach_tp(ebpf, "block_rq_issue", "block:block_rq_issue")?;
        self.attach_tp(ebpf, "block_rq_complete", "block:block_rq_complete")?;
        self.attach_tp(ebpf, "block_rq_requeue", "block_rq_requeue")?;
        self.attach_tp(ebpf, "block_bio_complete", "block_bio_complete")?;
        self.attach_tp(ebpf, "block_split", "block_split")?;
        self.attach_tp(ebpf, "block_unplug", "block_unplug")?;
        self.attach_tp(ebpf, "block_rq_abort", "block_rq_abort")?;
        
        // Memory tracepoints
        self.attach_tp(ebpf, "page_fault_user", "page_fault_user")?;
        self.attach_tp(ebpf, "page_fault_kernel", "page_fault_kernel")?;
        self.attach_tp(ebpf, "page_fault_enter", "page_fault_enter")?;
        self.attach_tp(ebpf, "page_fault_exit", "page_fault_exit")?;
        self.attach_tp(ebpf, "mm_page_alloc", "mm_page_alloc")?;
        self.attach_tp(ebpf, "mm_page_free", "mm_page_free")?;
        self.attach_tp(ebpf, "mm_vmscan_reclaim_pages", "mm_vmscan_reclaim_pages")?;
        self.attach_tp(ebpf, "mm_compaction_begin", "mm_compaction_begin")?;
        self.attach_tp(ebpf, "mm_compaction_end", "mm_compaction_end")?;
        self.attach_tp(ebpf, "mm_oom_kill", "mm_oom_kill")?;
        self.attach_tp(ebpf, "mm_slab_alloc", "mm_slab_alloc")?;
        
        // Kprobes for memory
        self.attach_kprobe(ebpf, "swap_readpage", "swap_readpage")?;
        self.attach_kretprobe(ebpf, "swap_readpage", "swap_readpage_ret")?;
        self.attach_kprobe(ebpf, "shrink_slab", "shrink_slab")?;
        
        Ok(())
    }
    
    fn attach_tp(&self, ebpf: &mut Ebpf, prog_name: &str, tp_name: &str) -> Result<()> {
        let program: &mut TracePoint = ebpf.program_mut(prog_name)
            .ok_or_else(|| Error::Ebpf(aya::EbpfError::ProgramNotFound(prog_name.to_string())))?
            .try_into()?;
        
        program.attach(tp_name, prog_name)
            .map_err(|e| Error::Ebpf(e))?;
        
        Ok(())
    }
    
    fn attach_kprobe(&self, ebpf: &mut Ebpf, prog_name: &str, func_name: &str) -> Result<()> {
        let program: &mut KProbe = ebpf.program_mut(prog_name)
            .ok_or_else(|| Error::Ebpf(aya::EbpfError::ProgramNotFound(prog_name.to_string())))?
            .try_into()?;
        
        program.attach(func_name, 0)
            .map_err(|e| Error::Ebpf(e))?;
        
        Ok(())
    }
    
    fn attach_kretprobe(&self, ebpf: &mut Ebpf, prog_name: &str, func_name: &str) -> Result<()> {
        let program: &mut KRetProbe = ebpf.program_mut(prog_name)
            .ok_or_else(|| Error::Ebpf(aya::EbpfError::ProgramNotFound(prog_name.to_string())))?
            .try_into()?;
        
        program.attach(func_name, 0)
            .map_err(|e| Error::Ebpf(e))?;
        
        Ok(())
    }
    
    async fn setup_ring_buffers(&self, ebpf: &Ebpf) -> Result<()> {
        let ring_buf_names = ["scheduler_events", "network_events", "block_events", "memory_events"];
        
        for name in &ring_buf_names {
            if let Some(MapData::RingBuf(rb)) = ebpf.map(name) {
                let rb = rb.try_clone()?;
                self.ring_buffers.lock().await.insert(name.to_string(), rb);
            }
        }
        
        Ok(())
    }
    
    /// Poll all ring buffers for events
    pub async fn poll_events(&self) -> Result<()> {
        let ebpf_guard = self.ebpf.lock().await;
        let ebpf = match ebpf_guard.as_ref() {
            Some(e) => e,
            None => return Ok(()),
        };
        
        let ring_buffers = self.ring_buffers.lock().await;
        
        for (name, rb) in ring_buffers.iter() {
            while let Some(item) = rb.next() {
                let data = item.as_slice();
                self.handle_event(name, data).await?;
            }
        }
        
        Ok(())
    }
    
    async fn handle_event(&self, source: &str, data: &[u8]) -> Result<()> {
        // Parse event based on source
        match source {
            "scheduler_events" => self.handle_scheduler_event(data).await?,
            "network_events" => self.handle_network_event(data).await?,
            "block_events" => self.handle_block_event(data).await?,
            "memory_events" => self.handle_memory_event(data).await?,
            _ => {}
        }
        Ok(())
    }
    
    async fn handle_scheduler_event(&self, data: &[u8]) -> Result<()> {
        // Parse scheduler event (simplified)
        if data.len() >= 8 {
            let timestamp = u64::from_le_bytes(data[0..8].try_into().unwrap_or([0;8]));
            self.emit_metric("scheduler", "events", 1.0).await?;
        }
        Ok(())
    }
    
    async fn handle_network_event(&self, data: &[u8]) -> Result<()> {
        if data.len() >= 8 {
            self.emit_metric("network", "events", 1.0).await?;
        }
        Ok(())
    }
    
    async fn handle_block_event(&self, data: &[u8]) -> Result<()> {
        if data.len() >= 8 {
            self.emit_metric("block", "events", 1.0).await?;
        }
        Ok(())
    }
    
    async fn handle_memory_event(&self, data: &[u8]) -> Result<()> {
        if data.len() >= 8 {
            self.emit_metric("memory", "events", 1.0).await?;
        }
        Ok(())
    }
    
    async fn emit_metric(&self, collector: &str, metric: &str, value: f64) -> Result<()> {
        let sample = MetricSample {
            collector: collector.to_string(),
            metric: metric.to_string(),
            value,
            timestamp: chrono::Utc::now(),
            device_id: self.device_id.clone(),
        };
        
        self.event_bus.send(Event::MetricSampled {
            collector: sample.collector,
            metric: sample.metric,
            value: sample.value,
            timestamp: sample.timestamp,
        })?;
        
        self.metrics.gauge(&format!("{}.{}", collector, metric), value);
        Ok(())
    }
    
    /// Read map values for current stats
    pub async fn read_stats(&self) -> Result<HashMap<String, f64>> {
        let mut stats = HashMap::new();
        let ebpf_guard = self.ebpf.lock().await;
        let ebpf = match ebpf_guard.as_ref() {
            Some(e) => e,
            None => return Ok(stats),
        };
        
        // Read per-CPU stats maps
        // This is simplified - real implementation would read per-CPU arrays
        
        Ok(stats)
    }
    
    /// Shutdown eBPF programs
    pub async fn shutdown(&self) {
        let mut ebpf_guard = self.ebpf.lock().await;
        if let Some(_ebpf) = ebpf_guard.take() {
            // Programs automatically detach on drop
        }
        self.ring_buffers.lock().await.clear();
        self.metrics.gauge("programs_loaded", 0.0);
    }
}