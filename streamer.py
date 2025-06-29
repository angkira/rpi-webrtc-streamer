#!/usr/bin/env python3
"""
Simple Python WebRTC Camera Streaming Server
Replaces the Rust service for rapid prototyping and testing.
"""

import asyncio
import json
import logging
import websockets
from aiortc import RTCPeerConnection, RTCSessionDescription, VideoStreamTrack
from aiortc.contrib.media import MediaPlayer
import cv2
import numpy as np
from av import VideoFrame

logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)


class CameraVideoTrack(VideoStreamTrack):
    """Video track that captures from camera using OpenCV"""

    def __init__(self, camera_index=0, width=640, height=480, fps=30):
        super().__init__()
        self.camera_index = camera_index
        self.width = width
        self.height = height
        self.fps = fps
        self.cap = None
        self._setup_camera()

    def _setup_camera(self):
        """Initialize camera with proper settings"""
        self.cap = cv2.VideoCapture(self.camera_index)
        self.cap.set(cv2.CAP_PROP_FRAME_WIDTH, self.width)
        self.cap.set(cv2.CAP_PROP_FRAME_HEIGHT, self.height)
        self.cap.set(cv2.CAP_PROP_FPS, self.fps)

        # Verify camera opened
        if not self.cap.isOpened():
            logger.error(f"Failed to open camera {self.camera_index}")
            raise RuntimeError(f"Camera {self.camera_index} not available")

        # Log actual resolution
        actual_width = int(self.cap.get(cv2.CAP_PROP_FRAME_WIDTH))
        actual_height = int(self.cap.get(cv2.CAP_PROP_FRAME_HEIGHT))
        actual_fps = self.cap.get(cv2.CAP_PROP_FPS)
        logger.info(
            f"Camera {self.camera_index}: {actual_width}x{actual_height} @ {actual_fps}fps"
        )

    async def recv(self):
        """Capture frame from camera and return as VideoFrame"""
        ret, frame = self.cap.read()
        if not ret:
            logger.warning("Failed to capture frame")
            # Return black frame on failure
            frame = np.zeros((self.height, self.width, 3), dtype=np.uint8)

        # Convert BGR to RGB (OpenCV uses BGR, WebRTC expects RGB)
        frame_rgb = cv2.cvtColor(frame, cv2.COLOR_BGR2RGB)

        # Create VideoFrame from numpy array
        video_frame = VideoFrame.from_ndarray(frame_rgb, format="rgb24")
        video_frame.pts = self.next_timestamp()
        video_frame.time_base = self.time_base

        return video_frame

    def __del__(self):
        if self.cap:
            self.cap.release()


class WebRTCServer:
    """WebRTC server handling camera streams"""

    def __init__(self, port=5557, camera_index=0):
        self.port = port
        self.camera_index = camera_index
        self.connections = set()

    async def handle_websocket(self, websocket, path):
        """Handle WebSocket connection for WebRTC signaling"""
        logger.info(f"New client connected from {websocket.remote_address}")

        # Create RTCPeerConnection
        pc = RTCPeerConnection()
        self.connections.add(pc)

        # Add camera video track
        try:
            video_track = CameraVideoTrack(camera_index=self.camera_index)
            pc.addTrack(video_track)
            logger.info(f"Added camera {self.camera_index} track to peer connection")
        except Exception as e:
            logger.error(f"Failed to add camera track: {e}")
            await websocket.send(json.dumps({"error": f"Camera error: {e}"}))
            return

        @pc.on("icecandidate")
        async def on_icecandidate(candidate):
            if candidate:
                logger.debug(f"Sending ICE candidate: {candidate.candidate}")
                await websocket.send(
                    json.dumps(
                        {
                            "iceCandidate": {
                                "candidate": candidate.candidate,
                                "sdpMLineIndex": candidate.sdpMLineIndex,
                                "sdpMid": candidate.sdpMid,
                            }
                        }
                    )
                )

        try:
            async for message in websocket:
                try:
                    data = json.loads(message)
                    logger.debug(f"Received: {list(data.keys())}")

                    if "offer" in data:
                        # Handle SDP offer
                        offer = RTCSessionDescription(
                            sdp=data["offer"]["sdp"], type=data["offer"]["type"]
                        )

                        # Set remote description
                        await pc.setRemoteDescription(offer)
                        logger.info("Set remote description (offer)")

                        # Create answer
                        answer = await pc.createAnswer()
                        await pc.setLocalDescription(answer)
                        logger.info("Created and set local description (answer)")

                        # Send answer back
                        await websocket.send(
                            json.dumps(
                                {"answer": {"type": answer.type, "sdp": answer.sdp}}
                            )
                        )
                        logger.info("Sent SDP answer")

                    elif "iceCandidate" in data:
                        # Handle ICE candidate
                        candidate_data = data["iceCandidate"]
                        await pc.addIceCandidate(candidate_data["candidate"])
                        logger.debug("Added ICE candidate")

                except json.JSONDecodeError:
                    logger.error("Invalid JSON received")
                except Exception as e:
                    logger.error(f"Error processing message: {e}")

        except websockets.exceptions.ConnectionClosed:
            logger.info("Client disconnected")
        except Exception as e:
            logger.error(f"WebSocket error: {e}")
        finally:
            # Cleanup
            await pc.close()
            self.connections.discard(pc)
            logger.info("Cleaned up peer connection")

    async def start(self):
        """Start the WebRTC server"""
        logger.info(f"Starting WebRTC server on port {self.port}")
        logger.info(f"Using camera index {self.camera_index}")

        # Test camera first
        try:
            test_track = CameraVideoTrack(camera_index=self.camera_index)
            del test_track
            logger.info("Camera test successful")
        except Exception as e:
            logger.error(f"Camera test failed: {e}")
            return

        # Start WebSocket server
        server = await websockets.serve(
            self.handle_websocket,
            "0.0.0.0",
            self.port,
            ping_interval=20,
            ping_timeout=10,
        )

        logger.info(f"WebRTC server listening on ws://0.0.0.0:{self.port}")
        await server.wait_closed()


def main():
    """Main entry point"""
    import argparse

    parser = argparse.ArgumentParser(description="Python WebRTC Camera Server")
    parser.add_argument("--port", type=int, default=5557, help="WebSocket port")
    parser.add_argument(
        "--camera", type=int, default=0, help="Camera index (0, 1, 2...)"
    )
    parser.add_argument("--debug", action="store_true", help="Enable debug logging")

    args = parser.parse_args()

    if args.debug:
        logging.getLogger().setLevel(logging.DEBUG)

    # Create and start server
    server = WebRTCServer(port=args.port, camera_index=args.camera)

    try:
        asyncio.run(server.start())
    except KeyboardInterrupt:
        logger.info("Server stopped by user")


if __name__ == "__main__":
    main()
