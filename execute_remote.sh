#!/bin/sh
host=$1
rsync -rz examples src Cargo.toml $host:speedy-http/
ssh $host "bash -s" <<"EOF"
cd ~/speedy-http
cargo build --example pooled_kucoin_tls --release
sudo tcpdump -w speedy_http.pcap port 443 &
cargo run --example pooled_kucoin_tls --release
sudo pkill tcpdump # kill does not work
EOF

scp $host:speedy-http/*.csv  ./
scp $host:speedy-http/*.pcap  ./
python3 plotting.py
