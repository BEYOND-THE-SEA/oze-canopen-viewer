use std::collections::{HashMap, VecDeque};
use tokio::time::Instant;

/// Detailed bus statistics tracker
#[derive(Debug, Clone)]
pub struct BusStats {
    // Message counting
    total_messages: u64,
    messages_history: VecDeque<(Instant, u64)>, // (timestamp, message_count)
    
    // Bus load tracking
    current_load: f64,
    peak_load: f64,
    avg_load: f64,
    load_samples: VecDeque<f64>,
    
    // Timing analysis
    last_message_time: Option<Instant>,
    min_gap: Option<f64>, // milliseconds
    max_gap: Option<f64>, // milliseconds
    gap_sum: f64,
    gap_count: u64,
    gap_history: VecDeque<f64>,
    
    // COB-ID frequency tracking
    cob_id_counts: HashMap<u16, u64>,
    cob_id_last_seen: HashMap<u16, Instant>,
    cob_id_rates: HashMap<u16, f64>, // Hz
    
    // Message rate
    current_msg_rate: f64, // messages per second
    peak_msg_rate: f64,
    avg_msg_rate: f64,
    
    // Start time for calculations
    start_time: Instant,
}

impl Default for BusStats {
    fn default() -> Self {
        Self::new()
    }
}

impl BusStats {
    pub fn new() -> Self {
        Self {
            total_messages: 0,
            messages_history: VecDeque::new(),
            current_load: 0.0,
            peak_load: 0.0,
            avg_load: 0.0,
            load_samples: VecDeque::new(),
            last_message_time: None,
            min_gap: None,
            max_gap: None,
            gap_sum: 0.0,
            gap_count: 0,
            gap_history: VecDeque::new(),
            cob_id_counts: HashMap::new(),
            cob_id_last_seen: HashMap::new(),
            cob_id_rates: HashMap::new(),
            current_msg_rate: 0.0,
            peak_msg_rate: 0.0,
            avg_msg_rate: 0.0,
            start_time: Instant::now(),
        }
    }
    
    /// Update statistics with a new message
    pub fn on_message(&mut self, cob_id: u16, timestamp: Instant) {
        self.total_messages += 1;
        
        // Update COB-ID count
        *self.cob_id_counts.entry(cob_id).or_insert(0) += 1;
        
        // Calculate inter-frame gap
        if let Some(last_time) = self.last_message_time {
            let gap_ms = (timestamp - last_time).as_secs_f64() * 1000.0;
            
            // Update min/max/avg gap
            self.min_gap = Some(self.min_gap.map_or(gap_ms, |min| min.min(gap_ms)));
            self.max_gap = Some(self.max_gap.map_or(gap_ms, |max| max.max(gap_ms)));
            self.gap_sum += gap_ms;
            self.gap_count += 1;
            
            // Keep gap history (last 1000 samples)
            self.gap_history.push_back(gap_ms);
            if self.gap_history.len() > 1000 {
                self.gap_history.pop_front();
            }
        }
        
        self.last_message_time = Some(timestamp);
        self.cob_id_last_seen.insert(cob_id, timestamp);
        
        // Update message history for rate calculation
        self.messages_history.push_back((timestamp, self.total_messages));
        // Keep only last 5 seconds of history
        while let Some((old_time, _)) = self.messages_history.front() {
            if timestamp.duration_since(*old_time).as_secs_f64() > 5.0 {
                self.messages_history.pop_front();
            } else {
                break;
            }
        }
    }
    
    /// Update bus load value
    pub fn update_load(&mut self, load: f64) {
        self.current_load = load;
        self.peak_load = self.peak_load.max(load);
        
        // Update average load
        self.load_samples.push_back(load);
        if self.load_samples.len() > 100 {
            self.load_samples.pop_front();
        }
        if !self.load_samples.is_empty() {
            self.avg_load = self.load_samples.iter().sum::<f64>() / self.load_samples.len() as f64;
        }
    }
    
    /// Calculate current message rate
    pub fn calculate_msg_rate(&mut self) {
        if self.messages_history.len() < 2 {
            self.current_msg_rate = 0.0;
            return;
        }
        
        if let (Some((first_time, first_count)), Some((last_time, last_count))) = 
            (self.messages_history.front(), self.messages_history.back()) {
            let duration = last_time.duration_since(*first_time).as_secs_f64();
            if duration > 0.0 {
                let msg_diff = last_count - first_count;
                self.current_msg_rate = msg_diff as f64 / duration;
                self.peak_msg_rate = self.peak_msg_rate.max(self.current_msg_rate);
                
                // Calculate average rate
                let total_duration = Instant::now().duration_since(self.start_time).as_secs_f64();
                if total_duration > 0.0 {
                    self.avg_msg_rate = self.total_messages as f64 / total_duration;
                }
            }
        }
    }
    
    /// Calculate rates for each COB-ID
    pub fn calculate_cob_id_rates(&mut self, now: Instant) {
        for (cob_id, _last_seen) in &self.cob_id_last_seen {
            if let Some(count) = self.cob_id_counts.get(cob_id) {
                let duration = now.duration_since(self.start_time).as_secs_f64();
                if duration > 1.0 {
                    let rate = *count as f64 / duration;
                    self.cob_id_rates.insert(*cob_id, rate);
                }
            }
        }
    }
    
    /// Get top N most frequent COB-IDs
    pub fn get_top_cob_ids(&self, n: usize) -> Vec<(u16, f64)> {
        let mut rates: Vec<_> = self.cob_id_rates.iter()
            .map(|(cob_id, rate)| (*cob_id, *rate))
            .collect();
        rates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        rates.truncate(n);
        rates
    }
    
    // Getters
    pub fn total_messages(&self) -> u64 { self.total_messages }
    pub fn current_load(&self) -> f64 { self.current_load }
    pub fn peak_load(&self) -> f64 { self.peak_load }
    pub fn avg_load(&self) -> f64 { self.avg_load }
    pub fn min_gap(&self) -> Option<f64> { self.min_gap }
    pub fn max_gap(&self) -> Option<f64> { self.max_gap }
    pub fn avg_gap(&self) -> Option<f64> {
        if self.gap_count > 0 {
            Some(self.gap_sum / self.gap_count as f64)
        } else {
            None
        }
    }
    pub fn jitter(&self) -> Option<f64> {
        if self.gap_history.len() < 2 {
            return None;
        }
        let avg = self.gap_history.iter().sum::<f64>() / self.gap_history.len() as f64;
        let variance = self.gap_history.iter()
            .map(|gap| (gap - avg).powi(2))
            .sum::<f64>() / self.gap_history.len() as f64;
        Some(variance.sqrt())
    }
    pub fn current_msg_rate(&self) -> f64 { self.current_msg_rate }
    pub fn peak_msg_rate(&self) -> f64 { self.peak_msg_rate }
    pub fn avg_msg_rate(&self) -> f64 { self.avg_msg_rate }
}

