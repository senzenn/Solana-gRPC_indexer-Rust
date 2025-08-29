# Solana Indexer CLI - High-Performance Blockchain Monitoring Tool

A powerful command-line tool for real-time Solana blockchain monitoring, account tracking, and data indexing with advanced caching and gRPC streaming capabilities.

<div align="center">
  <img src="public/solanaLogoMark.png" alt="Solana Logo" width="80" height="80" style="vertical-align: middle;">
</div>

![Solana Indexer](https://img.shields.io/badge/Solana-Indexer-blue?style=for-the-badge&logo=solana)
![Rust](https://img.shields.io/badge/Rust-000000?style=for-the-badge&logo=rust)

## 🚀 Key Features

- **Real-time Slot Tracking**: Monitor Solana slot progression with millisecond precision
- **Account & Wallet Monitoring**: Track balances, transactions, and activity for specific accounts
- **gRPC Streaming**: High-performance streaming API for real-time data delivery
- **Multi-Layer Caching**: LRU + TTL caching system with sub-millisecond response times
- **IPFS Integration**: Distributed storage for blockchain data persistence
- **Database Persistence**: SQLite database for historical data and analytics
- **Interactive TUI**: Beautiful terminal user interface with real-time dashboards
- **Performance Benchmarking**: Built-in performance testing and optimization

## 🖥️ Demo

### Home Interface
![Solana Indexer Home](public/home.png)
*Beautiful terminal interface with ASCII art logo and feature overview*

### Real-time Slot Tracking
![Slot Tracking Demo](public/slotv2.png)
*Real-time slot monitoring with horizontal layout and colored output*

## 📦 Installation

### Prerequisites
- Rust 1.70+
- SQLite3

### Quick Start
```bash
# Clone and build
git clone https://github.com/yourusername/solana-indexer.git
cd solana-indexer/cli-grpc
cargo build --release

# Set up environment
cp env.example .env
# Edit .env with your API keys

# Run the CLI
cargo run -- --help
```

## 🎯 Essential Commands

### Real-time Slot Tracking
```bash
# Track Solana slots with leader information
cargo run -- track slots --leaders --interval 400

# Track with detailed transaction information
cargo run -- track slots --transactions --save
```

### Account & Wallet Monitoring
```bash
# Add account to monitoring
cargo run -- track wallets add --address u5LGUD4bX7BpaUuMjNw5oZp1vcbJhhPy9dJpKaWggCX --name "My Account"

# Remove account from db
cargo run -- track wallets remove --address u5LGUD4bX7BpaUuMjNw5oZp1vcbJhhPy9dJpKaWggCX --name "My Account"


# Monitor account in real-time
cargo run -- track wallets watch --interval 2000

# List monitored accounts
cargo run -- track wallets list --detailed
```

### Interactive TUI (Borken) 
```bash
# Launch beautiful terminal interface
cargo run -- tui

# Professional logging interface
cargo run -- logger
```

### Performance Testing
```bash
# Run comprehensive performance tests
cargo run -- performance-benchmark --duration 60
```

## 🔧 Configuration

### Environment Variables
```bash
# Solana RPC endpoints
SOLANA_RPC_URL=https://api.mainnet-beta.solana.com
QUICKNODE_API_KEY=your_quicknode_api_key

# Database
DATABASE_URL=sqlite:solana_indexer.db
```

## 📊 Performance

- **L1 Cache**: Sub-millisecond access for hot slots
- **L2 Cache**: Millisecond access for recent transactions
- **L3 Cache**: Few milliseconds for account states
- **Throughput**: 1000+ slots per second, 5000+ TPS capacity
- **Response Time**: Sub-millisecond cache access

## 🏗️ Architecture

![Project Architecture](assets/image.png)

*Detailed architecture diagram showing the data flow from CLI interface through core services, data sources, caching & storage, to output & streaming layers.*

## 🏛️ Core Modules

- **Slot Tracker**: Real-time slot progression monitoring
- **Account Watcher**: Account balance and transaction monitoring
- **Cache System**: Multi-layer LRU + TTL caching
- **Database Layer**: SQLite persistence for historical data
- **gRPC Server**: High-performance streaming API
- **IPFS Storage**: Distributed data storage

## 🤝 Contributing

```bash
# Run tests
cargo test

# Run benchmarks
cargo bench
```

## 📄 License

MIT License - see the [LICENSE](LICENSE) file for details.

---


# solana-gprc-indexerRust
# rust-solana-grpc
