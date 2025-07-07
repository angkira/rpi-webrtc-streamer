package main

import (
	"context"
	"flag"
	"fmt"
	"os"
	"os/signal"
	"path/filepath"
	"runtime"
	"sort"
	"sync"
	"syscall"
	"time"

	"pi-camera-streamer/camera"
	"pi-camera-streamer/config"
	"pi-camera-streamer/web"
	"pi-camera-streamer/webrtc"
	"go.uber.org/zap"
	"go.uber.org/zap/zapcore"
)

const (
	DefaultConfigPath = "config.toml"
	AppName          = "Pi Camera WebRTC Streamer"
	AppVersion       = "1.0.0"
)

// Application represents the main application
type Application struct {
	config        *config.Config
	logger        *zap.Logger
	
	// Components
	cameraManager *camera.Manager
	webrtcServers map[string]*webrtc.Server
	webServer     *web.Server
	
	// Lifecycle
	ctx           context.Context
	cancel        context.CancelFunc
	wg            sync.WaitGroup
}

func main() {
	// Parse command line flags
	var (
		configPath = flag.String("config", DefaultConfigPath, "Path to configuration file")
		logLevel   = flag.String("log-level", "info", "Log level (debug, info, warn, error)")
		version    = flag.Bool("version", false, "Show version information")
		help       = flag.Bool("help", false, "Show help information")
	)
	flag.Parse()

	if *version {
		fmt.Printf("%s v%s\n", AppName, AppVersion)
		fmt.Printf("Go version: %s\n", runtime.Version())
		fmt.Printf("Platform: %s/%s\n", runtime.GOOS, runtime.GOARCH)
		os.Exit(0)
	}

	if *help {
		fmt.Printf("%s v%s\n\n", AppName, AppVersion)
		fmt.Println("A high-performance WebRTC streaming service for Raspberry Pi dual cameras")
		fmt.Println("\nUsage:")
		flag.PrintDefaults()
		fmt.Println("\nEnvironment Variables:")
		fmt.Println("  PI_IP - Override auto-detected Pi IP address")
		os.Exit(0)
	}

	// Create logger
	logger, err := createLogger(*logLevel)
	if err != nil {
		fmt.Printf("Failed to create logger: %v\n", err)
		os.Exit(1)
	}
	defer logger.Sync()

	logger.Info("Starting Pi Camera WebRTC Streamer",
		zap.String("version", AppVersion),
		zap.String("go_version", runtime.Version()),
		zap.String("platform", runtime.GOOS+"/"+runtime.GOARCH))

	// Load configuration
	cfg, err := config.LoadConfig(*configPath)
	if err != nil {
		logger.Fatal("Failed to load configuration", zap.Error(err))
	}

	// Override PI IP from environment if set
	if envIP := os.Getenv("PI_IP"); envIP != "" {
		cfg.Server.PIIp = envIP
		logger.Info("PI IP overridden from environment", zap.String("ip", envIP))
	}

	logger.Info("Configuration loaded",
		zap.String("pi_ip", cfg.Server.PIIp),
		zap.Int("web_port", cfg.Server.WebPort),
		zap.Int("camera1_port", cfg.Camera1.WebRTCPort),
		zap.Int("camera2_port", cfg.Camera2.WebRTCPort))

	// Create application
	app := NewApplication(cfg, logger)

	// Set up signal handling
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	signalCh := make(chan os.Signal, 1)
	signal.Notify(signalCh, os.Interrupt, syscall.SIGTERM)

	// Start application
	if err := app.Start(ctx); err != nil {
		logger.Fatal("Failed to start application", zap.Error(err))
	}

	// Wait for shutdown signal
	select {
	case sig := <-signalCh:
		logger.Info("Received shutdown signal", zap.String("signal", sig.String()))
	case <-ctx.Done():
		logger.Info("Context cancelled")
	}

	// Graceful shutdown
	logger.Info("Shutting down...")
	shutdownCtx, shutdownCancel := context.WithTimeout(context.Background(), time.Duration(app.config.Timeouts.ShutdownTimeout)*time.Second)
	defer shutdownCancel()

	if err := app.Stop(shutdownCtx); err != nil {
		logger.Error("Error during shutdown", zap.Error(err))
		os.Exit(1)
	}

	logger.Info("Shutdown complete")
}

// NewApplication creates a new application instance
func NewApplication(cfg *config.Config, logger *zap.Logger) *Application {
	ctx, cancel := context.WithCancel(context.Background())
	
	return &Application{
		config:        cfg,
		logger:        logger,
		webrtcServers: make(map[string]*webrtc.Server),
		ctx:           ctx,
		cancel:        cancel,
	}
}

// Start starts all application components
func (a *Application) Start(ctx context.Context) error {
	a.logger.Info("Starting application components")

	// Initialize camera manager
	if err := a.initializeCameraManager(); err != nil {
		return fmt.Errorf("failed to initialize camera manager: %w", err)
	}

	// Initialize WebRTC servers
	if err := a.initializeWebRTCServers(); err != nil {
		return fmt.Errorf("failed to initialize WebRTC servers: %w", err)
	}

	// Initialize web server
	if err := a.initializeWebServer(); err != nil {
		return fmt.Errorf("failed to initialize web server: %w", err)
	}

	// Start all components
	if err := a.startComponents(); err != nil {
		return fmt.Errorf("failed to start components: %w", err)
	}

	a.logger.Info("Application started successfully",
		zap.String("web_url", fmt.Sprintf("http://%s:%d", a.config.Server.PIIp, a.config.Server.WebPort)),
		zap.String("webrtc_cam1", fmt.Sprintf("ws://%s:%d/ws", a.config.Server.PIIp, a.config.Camera1.WebRTCPort)),
		zap.String("webrtc_cam2", fmt.Sprintf("ws://%s:%d/ws", a.config.Server.PIIp, a.config.Camera2.WebRTCPort)))

	return nil
}

// initializeCameraManager sets up the camera management system
func (a *Application) initializeCameraManager() error {
	a.logger.Info("Initializing camera manager")

	a.cameraManager = camera.NewManager(a.config, a.logger)

	// Initialize cameras
	for _, cameraID := range []string{"camera1", "camera2"} {
		if err := a.cameraManager.InitializeCamera(cameraID); err != nil {
			a.logger.Warn("Failed to initialize camera", zap.String("camera", cameraID), zap.Error(err))
		}
	}

	a.logger.Info("Camera manager initialized")
	return nil
}

// initializeWebRTCServers creates WebRTC servers for each camera
func (a *Application) initializeWebRTCServers() error {
	a.logger.Info("Initializing WebRTC servers")

	// Camera 1 WebRTC server
	server1, err := webrtc.NewServer("camera1", a.config.Camera1.WebRTCPort, a.config, a.logger)
	if err != nil {
		return fmt.Errorf("failed to create WebRTC server for camera1: %w", err)
	}

	// Set camera for server1
	if camera1, err := a.cameraManager.GetCamera("camera1"); err == nil {
		server1.SetCamera(camera1)
	}

	a.webrtcServers["camera1"] = server1

	// Camera 2 WebRTC server
	server2, err := webrtc.NewServer("camera2", a.config.Camera2.WebRTCPort, a.config, a.logger)
	if err != nil {
		return fmt.Errorf("failed to create WebRTC server for camera2: %w", err)
	}

	// Set camera for server2
	if camera2, err := a.cameraManager.GetCamera("camera2"); err == nil {
		server2.SetCamera(camera2)
	}

	a.webrtcServers["camera2"] = server2

	a.logger.Info("WebRTC servers initialized")
	return nil
}

// initializeWebServer creates the main web server
func (a *Application) initializeWebServer() error {
	a.logger.Info("Initializing web server")

	a.webServer = web.NewServer(a.config, a.logger)
	a.webServer.SetCameraManager(a.cameraManager)
	a.webServer.SetWebRTCServers(a.webrtcServers)

	a.logger.Info("Web server initialized")
	return nil
}

// startComponents starts all application components
func (a *Application) startComponents() error {
	a.logger.Info("Starting application components")

	// Start WebRTC servers
	for id, server := range a.webrtcServers {
		if err := server.Start(); err != nil {
			return fmt.Errorf("failed to start WebRTC server %s: %w", id, err)
		}
		a.logger.Info("WebRTC server started", zap.String("camera", id), zap.Int("port", server.GetPort()))
	}

	// Start web server
	if err := a.webServer.Start(); err != nil {
		return fmt.Errorf("failed to start web server: %w", err)
	}

	// Start cameras
	a.wg.Add(1)
	go a.startCamerasAsync()

	return nil
}

// startCamerasAsync starts cameras asynchronously
func (a *Application) startCamerasAsync() {
	defer a.wg.Done()

	// Wait a bit for WebRTC servers to be ready
	time.Sleep(time.Duration(a.config.Timeouts.WebRTCStartupDelay) * time.Millisecond)

	a.logger.Info("Starting cameras")
	
	for _, cameraID := range []string{"camera1", "camera2"} {
		if err := a.cameraManager.StartCamera(cameraID); err != nil {
			a.logger.Error("Failed to start camera", zap.String("camera", cameraID), zap.Error(err))
		} else {
			a.logger.Info("Camera started", zap.String("camera", cameraID))
		}
		// Add a delay between starting cameras to avoid resource conflicts
		time.Sleep(time.Duration(a.config.Timeouts.CameraStartupDelay) * time.Millisecond)
	}
}

// Stop gracefully stops all application components
func (a *Application) Stop(ctx context.Context) error {
	a.logger.Info("Stopping application")

	// Cancel context
	a.cancel()

	// Stop web server
	if a.webServer != nil {
		if err := a.webServer.Stop(); err != nil {
			a.logger.Error("Error stopping web server", zap.Error(err))
		}
	}

	// Stop WebRTC servers
	for id, server := range a.webrtcServers {
		if err := server.Stop(); err != nil {
			a.logger.Error("Error stopping WebRTC server", zap.String("camera", id), zap.Error(err))
		}
	}

	// Stop camera manager
	if a.cameraManager != nil {
		if err := a.cameraManager.Close(); err != nil {
			a.logger.Error("Error stopping camera manager", zap.Error(err))
		}
	}

	// Wait for goroutines to finish
	done := make(chan struct{})
	go func() {
		a.wg.Wait()
		close(done)
	}()

	select {
	case <-done:
		a.logger.Info("All components stopped gracefully")
	case <-ctx.Done():
		a.logger.Warn("Shutdown timeout reached, forcing exit")
	}

	return nil
}

// createLogger creates a structured logger
func createLogger(level string) (*zap.Logger, error) {
	var zapLevel zapcore.Level
	switch level {
	case "debug":
		zapLevel = zapcore.DebugLevel
	case "info":
		zapLevel = zapcore.InfoLevel
	case "warn":
		zapLevel = zapcore.WarnLevel
	case "error":
		zapLevel = zapcore.ErrorLevel
	default:
		zapLevel = zapcore.InfoLevel
	}

	// Prepare log directory and file path
	const logDir = "logs"
	if err := os.MkdirAll(logDir, 0755); err != nil {
		return nil, fmt.Errorf("failed to create log dir: %w", err)
	}
	ts := time.Now().Format("20060102-150405")
	logFile := filepath.Join(logDir, fmt.Sprintf("pi-camera-streamer-%s.log", ts))

	// Clean up old logs (keep last 20 files)
	files, _ := filepath.Glob(filepath.Join(logDir, "pi-camera-streamer-*.log"))
	if len(files) > 20 {
		sort.Strings(files) // lexicographic order matches timestamp
		for _, f := range files[:len(files)-20] {
			_ = os.Remove(f)
		}
	}

	config := zap.Config{
		Level:       zap.NewAtomicLevelAt(zapLevel),
		Development: false,
		Sampling: &zap.SamplingConfig{
			Initial:    100,
			Thereafter: 100,
		},
		Encoding: "console",
		EncoderConfig: zapcore.EncoderConfig{
			TimeKey:        "timestamp",
			LevelKey:       "level",
			NameKey:        "logger",
			CallerKey:      "caller",
			MessageKey:     "msg",
			StacktraceKey:  "stacktrace",
			LineEnding:     zapcore.DefaultLineEnding,
			EncodeLevel:    zapcore.CapitalColorLevelEncoder,
			EncodeTime:     zapcore.ISO8601TimeEncoder,
			EncodeDuration: zapcore.SecondsDurationEncoder,
			EncodeCaller:   zapcore.ShortCallerEncoder,
		},
		OutputPaths:      []string{"stdout", logFile},
		ErrorOutputPaths: []string{"stderr", logFile},
	}

	return config.Build()
} 