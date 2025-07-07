package config

import (
	"fmt"
	"net"
	"os"

	"github.com/BurntSushi/toml"
	"go.uber.org/zap"
)

// Config represents the application configuration
type Config struct {
	Camera1  CameraConfig  `toml:"camera1" json:"camera1"`
	Camera2  CameraConfig  `toml:"camera2" json:"camera2"`
	Server   ServerConfig  `toml:"server" json:"server"`
	Encoding EncodingConfig `toml:"encoding" json:"encoding"`
	Video    VideoConfig   `toml:"video" json:"video"`
	WebRTC   WebRTCConfig  `toml:"webrtc" json:"webrtc"`
	Buffers  BufferConfig  `toml:"buffers" json:"buffers"`
	Timeouts TimeoutConfig `toml:"timeouts" json:"timeouts"`
	Logging  LoggingConfig `toml:"logging" json:"logging"`
	Limits   LimitConfig   `toml:"limits" json:"limits"`
}

// CameraConfig holds camera-specific settings
type CameraConfig struct {
	Device      string `toml:"device" json:"device"`
	Width       int    `toml:"width" json:"width"`
	Height      int    `toml:"height" json:"height"`
	FPS         int    `toml:"fps" json:"fps"`
	WebRTCPort  int    `toml:"webrtc_port" json:"webrtc_port"`
	FlipMethod  string `toml:"flip_method" json:"flip_method"`
}

// ServerConfig holds web server settings
type ServerConfig struct {
	WebPort int    `toml:"web_port" json:"web_port"`
	BindIP  string `toml:"bind_ip" json:"bind_ip"`
	PIIp    string `toml:"pi_ip" json:"pi_ip"` // Auto-detected if empty
}

// EncodingConfig holds video encoding settings
type EncodingConfig struct {
	Codec            string `toml:"codec" json:"codec"`
	Bitrate          int    `toml:"bitrate" json:"bitrate"`
	KeyframeInterval int    `toml:"keyframe_interval" json:"keyframe_interval"`
	CPUUsed          int    `toml:"cpu_used" json:"cpu_used"`
}

// VideoConfig holds general video settings (e.g., global codec choice for capture)
type VideoConfig struct {
	Codec           string `toml:"codec" json:"codec"`
	EncoderPreset   string `toml:"encoder-preset" json:"encoder_preset"`
	KeyframeInterval int    `toml:"keyframe-interval" json:"keyframe_interval"`
	CPUUsed         int    `toml:"cpu-used" json:"cpu_used"`
	Bitrate          int    `toml:"bitrate" json:"bitrate"`
}

// WebRTCConfig holds WebRTC-specific settings
type WebRTCConfig struct {
	STUNServer   string `toml:"stun_server" json:"stun_server"`
	MaxClients   int    `toml:"max_clients" json:"max_clients"`
	MTU          int    `toml:"mtu" json:"mtu"`
	Latency      int    `toml:"latency" json:"latency"`
	Timeout      int    `toml:"timeout" json:"timeout"`
}

// BufferConfig holds buffer size settings for channels
type BufferConfig struct {
	FrameChannelSize    int `toml:"frame_channel_size" json:"frame_channel_size"`
	EncodedChannelSize  int `toml:"encoded_channel_size" json:"encoded_channel_size"`
	SignalChannelSize   int `toml:"signal_channel_size" json:"signal_channel_size"`
	ErrorChannelSize    int `toml:"error_channel_size" json:"error_channel_size"`
}

// TimeoutConfig holds timeout and delay settings
type TimeoutConfig struct {
	WebRTCStartupDelay    int `toml:"webrtc_startup_delay_ms" json:"webrtc_startup_delay_ms"`
	CameraStartupDelay    int `toml:"camera_startup_delay_ms" json:"camera_startup_delay_ms"`
	EncoderSleepInterval  int `toml:"encoder_sleep_interval_ms" json:"encoder_sleep_interval_ms"`
	ShutdownTimeout       int `toml:"shutdown_timeout_seconds" json:"shutdown_timeout_seconds"`
	HTTPShutdownTimeout   int `toml:"http_shutdown_timeout_seconds" json:"http_shutdown_timeout_seconds"`
}

// LoggingConfig holds logging interval settings
type LoggingConfig struct {
	FrameLogInterval      int `toml:"frame_log_interval" json:"frame_log_interval"`
	StatsLogInterval      int `toml:"stats_log_interval_seconds" json:"stats_log_interval_seconds"`
}

// LimitConfig holds resource limit settings
type LimitConfig struct {
	MaxMemoryUsageMB     int `toml:"max_memory_usage_mb" json:"max_memory_usage_mb"`
	MaxLogFiles          int `toml:"max_log_files" json:"max_log_files"`
	MaxPayloadSizeMB     int `toml:"max_payload_size_mb" json:"max_payload_size_mb"`
}

// LoadConfig loads configuration from a TOML file
func LoadConfig(configPath string) (*Config, error) {
	logger, _ := zap.NewProduction()
	defer logger.Sync()

	// Set default values
	config := &Config{
		Camera1: CameraConfig{
			Device:     "/base/axi/pcie@1000120000/rp1/i2c@88000/imx219@10",
			Width:      640,
			Height:     480,
			FPS:        30,
			WebRTCPort: 5557,
			FlipMethod: "rotate-180",
		},
		Camera2: CameraConfig{
			Device:     "/base/axi/pcie@1000120000/rp1/i2c@80000/imx219@10",
			Width:      640,
			Height:     480,
			FPS:        30,
			WebRTCPort: 5558,
			FlipMethod: "rotate-180",
		},
		Server: ServerConfig{
			WebPort: 8080,
			BindIP:  "0.0.0.0",
		},
		Encoding: EncodingConfig{
			Codec:            "vp8",
			Bitrate:          2000000,
			KeyframeInterval: 30,
			CPUUsed:          8,
		},
		WebRTC: WebRTCConfig{
			STUNServer: "stun:stun.l.google.com:19302",
			MaxClients: 4,
			MTU:        1200,
			Latency:    200,
			Timeout:    10000,
		},
		Video: VideoConfig{
			Codec:           "h264",
			EncoderPreset:   "ultrafast",
			KeyframeInterval: 30,
			CPUUsed:          8,
			Bitrate:          2000000,
		},
		Buffers: BufferConfig{
			FrameChannelSize:    30,
			EncodedChannelSize:  20,
			SignalChannelSize:   1,
			ErrorChannelSize:    1,
		},
		Timeouts: TimeoutConfig{
			WebRTCStartupDelay:    2000,
			CameraStartupDelay:    1000,
			EncoderSleepInterval:  10,
			ShutdownTimeout:        30,
			HTTPShutdownTimeout:    5,
		},
		Logging: LoggingConfig{
			FrameLogInterval:     30,
			StatsLogInterval:     60,
		},
		Limits: LimitConfig{
			MaxMemoryUsageMB:     512,
			MaxLogFiles:          20,
			MaxPayloadSizeMB:      2,
		},
	}

	// Load from file if it exists
	if _, err := os.Stat(configPath); err == nil {
		if _, err := toml.DecodeFile(configPath, config); err != nil {
			return nil, fmt.Errorf("failed to decode config file: %w", err)
		}
		logger.Info("Config loaded from file", zap.String("path", configPath))
	} else {
		logger.Info("Config file not found, using defaults", zap.String("path", configPath))
	}

	// Auto-detect PI IP if not set
	if config.Server.PIIp == "" {
		if ip := getLocalIP(); ip != "" {
			config.Server.PIIp = ip
			logger.Info("Auto-detected PI IP", zap.String("ip", ip))
		} else {
			config.Server.PIIp = "localhost"
			logger.Warn("Could not detect PI IP, using localhost")
		}
	}

	return config, nil
}

// getLocalIP attempts to determine the local IP address
func getLocalIP() string {
	// Try to connect to a remote address to determine local IP
	conn, err := net.Dial("udp", "8.8.8.8:80")
	if err != nil {
		return ""
	}
	defer conn.Close()

	localAddr := conn.LocalAddr().(*net.UDPAddr)
	return localAddr.IP.String()
}

// SaveConfig saves the current configuration to a file
func SaveConfig(config *Config, configPath string) error {
	file, err := os.Create(configPath)
	if err != nil {
		return fmt.Errorf("failed to create config file: %w", err)
	}
	defer file.Close()

	encoder := toml.NewEncoder(file)
	if err := encoder.Encode(config); err != nil {
		return fmt.Errorf("failed to encode config: %w", err)
	}

	return nil
} 