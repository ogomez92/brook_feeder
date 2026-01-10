# Systemd Installation

## Install

```bash
# Copy binary and config
sudo mkdir -p /opt/feeder
sudo cp target/release/feeder /opt/feeder/
sudo cp .env /opt/feeder/
sudo cp feeder.db /opt/feeder/  # if you have existing feeds

# Install service files
sudo cp services/feeder.service /etc/systemd/system/
sudo cp services/feeder.timer /etc/systemd/system/

# Enable and start timer
sudo systemctl daemon-reload
sudo systemctl enable feeder.timer
sudo systemctl start feeder.timer
```

## Commands

```bash
# Check timer status
systemctl status feeder.timer

# Run manually
sudo systemctl start feeder.service

# View logs
journalctl -u feeder.service

# Stop timer
sudo systemctl stop feeder.timer
```
