use anyhow::Result;
use colored::*;
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{info, debug, warn, error};

use crate::config::Config;

/// IPFS storage configuration
#[derive(Debug, Clone)]
pub struct IpfsConfig {
    pub api_url: String,
    pub gateway_url: String,
    pub pin_enabled: bool,
    pub max_file_size: usize,
    pub compression_enabled: bool,
    pub encryption_enabled: bool,
}

/// IPFS file metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpfsFile {
    pub cid: String,
    pub name: String,
    pub size: usize,
    pub mime_type: String,
    pub uploaded_at: u64,
    pub pinned: bool,
    pub encrypted: bool,
    pub checksum: String,
}

/// IPFS storage statistics
#[derive(Debug, Clone)]
pub struct IpfsStats {
    pub total_files: usize,
    pub total_size: usize,
    pub pinned_files: usize,
    pub encrypted_files: usize,
    pub upload_success_rate: f64,
    pub average_upload_time: Duration,
}

/// High-performance IPFS storage system for blockchain data
pub struct IpfsStorage {
    config: IpfsConfig,
    files: HashMap<String, IpfsFile>,
    stats: IpfsStats,
    client: reqwest::Client,
}

impl IpfsStorage {
    pub fn new(config: IpfsConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default();

        Self {
            config,
            files: HashMap::new(),
            stats: IpfsStats {
                total_files: 0,
                total_size: 0,
                pinned_files: 0,
                encrypted_files: 0,
                upload_success_rate: 1.0,
                average_upload_time: Duration::ZERO,
            },
            client,
        }
    }

    /// Upload data to IPFS
    pub async fn upload_data(&mut self, name: &str, data: &[u8], mime_type: &str) -> Result<String> {
        let start_time = std::time::Instant::now();

        info!("{} {} | Uploading {} to IPFS (size: {} bytes)",
            "📤".bright_blue(),
            "IPFS_UPLOAD".bright_blue(),
            name.bright_cyan(),
            data.len().to_string().bright_yellow()
        );

        // Check file size limit
        if data.len() > self.config.max_file_size {
            return Err(anyhow::anyhow!("File size {} exceeds limit {}", data.len(), self.config.max_file_size));
        }

        // Compress data if enabled
        let processed_data = if self.config.compression_enabled {
            self.compress_data(data).await?
        } else {
            data.to_vec()
        };

        // Encrypt data if enabled
        let final_data = if self.config.encryption_enabled {
            self.encrypt_data(&processed_data).await?
        } else {
            processed_data
        };

        // Calculate checksum
        let checksum = self.calculate_checksum(&final_data);

        // Upload to IPFS (simulated for now)
        let cid = self.simulate_ipfs_upload(&final_data).await?;

        // Create file metadata
        let file = IpfsFile {
            cid: cid.clone(),
            name: name.to_string(),
            size: data.len(),
            mime_type: mime_type.to_string(),
            uploaded_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            pinned: self.config.pin_enabled,
            encrypted: self.config.encryption_enabled,
            checksum,
        };

        // Store file metadata
        self.files.insert(cid.clone(), file.clone());

        // Update statistics
        self.update_stats(data.len(), start_time.elapsed(), true);

        // Pin file if enabled
        if self.config.pin_enabled {
            self.pin_file(&cid).await?;
        }

        info!("{} {} | Successfully uploaded {} to IPFS (CID: {}...)",
            "✅".bright_green(),
            "IPFS_UPLOAD_SUCCESS".bright_green(),
            name.bright_cyan(),
            &cid[..12].bright_green()
        );

        Ok(cid)
    }

    /// Download data from IPFS
    pub async fn download_data(&self, cid: &str) -> Result<Vec<u8>> {
        let start_time = std::time::Instant::now();

        debug!("{} {} | Downloading data from IPFS (CID: {}...)",
            "📥".bright_blue(),
            "IPFS_DOWNLOAD".bright_blue(),
            &cid[..12].bright_cyan()
        );

        // Check if we have metadata
        if let Some(file) = self.files.get(cid) {
            debug!("{} {} | Found local metadata for {}",
                "📋".bright_blue(),
                "IPFS_METADATA".bright_blue(),
                file.name.bright_cyan()
            );
        }

        // Download from IPFS (simulated for now)
        let encrypted_data = self.simulate_ipfs_download(cid).await?;

        // Decrypt if needed
        let compressed_data = if self.config.encryption_enabled {
            self.decrypt_data(&encrypted_data).await?
        } else {
            encrypted_data
        };

        // Decompress if needed
        let original_data = if self.config.compression_enabled {
            self.decompress_data(&compressed_data).await?
        } else {
            compressed_data
        };

        let duration = start_time.elapsed();
        debug!("{} {} | Downloaded {} bytes in {:?}",
            "✅".bright_green(),
            "IPFS_DOWNLOAD_SUCCESS".bright_green(),
            original_data.len().to_string().bright_yellow(),
            duration
        );

        Ok(original_data)
    }

    /// Pin a file to keep it available
    pub async fn pin_file(&self, cid: &str) -> Result<()> {
        if !self.config.pin_enabled {
            return Ok(());
        }

        debug!("{} {} | Pinning file {}...",
            "📌".bright_blue(),
            "IPFS_PIN".bright_blue(),
            &cid[..12].bright_cyan()
        );

        // Simulate IPFS pinning
        tokio::time::sleep(Duration::from_millis(100)).await;

        debug!("{} {} | File {} pinned successfully",
            "✅".bright_green(),
            "IPFS_PIN_SUCCESS".bright_green(),
            &cid[..12].bright_cyan()
        );

        Ok(())
    }

    /// Unpin a file
    pub async fn unpin_file(&self, cid: &str) -> Result<()> {
        debug!("{} {} | Unpinning file {}...",
            "📌".bright_yellow(),
            "IPFS_UNPIN".bright_yellow(),
            &cid[..12].bright_cyan()
        );

        // Simulate IPFS unpinning
        tokio::time::sleep(Duration::from_millis(50)).await;

        debug!("{} {} | File {} unpinned successfully",
            "✅".bright_green(),
            "IPFS_UNPIN_SUCCESS".bright_green(),
            &cid[..12].bright_cyan()
        );

        Ok(())
    }

    /// Get file information
    pub fn get_file_info(&self, cid: &str) -> Option<&IpfsFile> {
        self.files.get(cid)
    }

    /// List all files
    pub fn list_files(&self) -> Vec<&IpfsFile> {
        self.files.values().collect()
    }

    /// Get storage statistics
    pub fn get_stats(&self) -> &IpfsStats {
        &self.stats
    }

    /// Clean up old files
    pub async fn cleanup_old_files(&mut self, max_age_days: u64) -> Result<usize> {
        let cutoff_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() - (max_age_days * 24 * 3600);

        let mut removed_count = 0;
        let mut to_remove = Vec::new();

        for (cid, file) in &self.files {
            if file.uploaded_at < cutoff_time && !file.pinned {
                to_remove.push(cid.clone());
            }
        }

        for cid in to_remove {
            if let Some(file) = self.files.remove(&cid) {
                // Unpin from IPFS
                self.unpin_file(&cid).await?;

                // Update statistics
                self.stats.total_files -= 1;
                self.stats.total_size = self.stats.total_size.saturating_sub(file.size);
                if file.encrypted {
                    self.stats.encrypted_files -= 1;
                }

                removed_count += 1;

                debug!("{} {} | Cleaned up old file: {} (CID: {}...)",
                    "🧹".bright_blue(),
                    "IPFS_CLEANUP".bright_blue(),
                    file.name.bright_cyan(),
                    &cid[..12].bright_cyan()
                );
            }
        }

        if removed_count > 0 {
            info!("{} {} | Cleanup completed: {} files removed",
                "✅".bright_green(),
                "IPFS_CLEANUP_SUCCESS".bright_green(),
                removed_count.to_string().bright_yellow()
            );
        }

        Ok(removed_count)
    }

    /// Compress data using gzip
    async fn compress_data(&self, data: &[u8]) -> Result<Vec<u8>> {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data)?;
        Ok(encoder.finish()?)
    }

    /// Decompress data using gzip
    async fn decompress_data(&self, data: &[u8]) -> Result<Vec<u8>> {
        use flate2::read::GzDecoder;
        use std::io::Read;

        let mut decoder = GzDecoder::new(data);
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed)?;
        Ok(decompressed)
    }

    /// Encrypt data (simplified for demo)
    async fn encrypt_data(&self, data: &[u8]) -> Result<Vec<u8>> {
        // In production, use proper encryption like AES-256
        // For demo, just XOR with a simple key
        let key = b"IPFS_ENCRYPTION_KEY_2024";
        let mut encrypted = Vec::with_capacity(data.len());

        for (i, &byte) in data.iter().enumerate() {
            encrypted.push(byte ^ key[i % key.len()]);
        }

        Ok(encrypted)
    }

    /// Decrypt data (simplified for demo)
    async fn decrypt_data(&self, data: &[u8]) -> Result<Vec<u8>> {
        // Same XOR operation for decryption
        self.encrypt_data(data).await
    }

    /// Calculate SHA-256 checksum
    fn calculate_checksum(&self, data: &[u8]) -> String {
        use sha2::{Sha256, Digest};

        let mut hasher = Sha256::new();
        hasher.update(data);
        format!("{:x}", hasher.finalize())
    }

    /// Simulate IPFS upload
    async fn simulate_ipfs_upload(&self, data: &[u8]) -> Result<String> {
        // Simulate network delay
        tokio::time::sleep(Duration::from_millis(200)).await;

        // Generate a realistic CID (Content Identifier)
        let mut hasher = sha2::Sha256::new();
        hasher.update(data);
        let hash = hasher.finalize();

        // Format as CID v1 (base32)
        let cid = format!("bafybeib{}", base32::encode(base32::Alphabet::RFC4648 { padding: false }, &hash[..20]));

        Ok(cid)
    }

    /// Simulate IPFS download
    async fn simulate_ipfs_download(&self, _cid: &str) -> Result<Vec<u8>> {
        // Simulate network delay
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Return sample data (in production, this would fetch from IPFS)
        Ok(b"Sample blockchain data from IPFS".to_vec())
    }

    /// Update statistics
    fn update_stats(&mut self, file_size: usize, upload_time: Duration, success: bool) {
        self.stats.total_files += 1;
        self.stats.total_size += file_size;

        if success {
            // Update success rate
            let total_requests = self.stats.total_files as f64;
            let current_success_rate = self.stats.upload_success_rate;
            self.stats.upload_success_rate = (current_success_rate * (total_requests - 1.0) + 1.0) / total_requests;

            // Update average upload time
            let total_time = self.stats.average_upload_time * (self.stats.total_files - 1) as u32 + upload_time;
            self.stats.average_upload_time = total_time / self.stats.total_files as u32;
        } else {
            // Update success rate for failure
            let total_requests = self.stats.total_files as f64;
            let current_success_rate = self.stats.upload_success_rate;
            self.stats.upload_success_rate = (current_success_rate * (total_requests - 1.0)) / total_requests;
        }
    }
}

impl Default for IpfsConfig {
    fn default() -> Self {
        Self {
            api_url: "http://localhost:5001".to_string(),
            gateway_url: "https://ipfs.io/ipfs/".to_string(),
            pin_enabled: true,
            max_file_size: 100 * 1024 * 1024, // 100MB
            compression_enabled: true,
            encryption_enabled: false, // Disabled by default for demo
        }
    }
}

/// Blockchain data types for IPFS storage
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlockchainData {
    pub data_type: String,
    pub slot: u64,
    pub timestamp: u64,
    pub data: serde_json::Value,
    pub metadata: HashMap<String, String>,
}

impl BlockchainData {
    pub fn new(data_type: &str, slot: u64, data: serde_json::Value) -> Self {
        Self {
            data_type: data_type.to_string(),
            slot,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            data,
            metadata: HashMap::new(),
        }
    }

    pub fn add_metadata(&mut self, key: &str, value: &str) {
        self.metadata.insert(key.to_string(), value.to_string());
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Ok(serde_json::from_slice(bytes)?)
    }
}
