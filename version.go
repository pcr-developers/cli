package main

// Version is injected at build time via:
// go build -ldflags="-X main.Version=1.2.3"
var Version = "dev"
