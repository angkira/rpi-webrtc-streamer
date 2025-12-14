# MJPEG-RTP Client Implementation Specification

## Purpose
Receive MJPEG-RTP streams (RFC 2435) from multiple cameras and deliver RGB frames for visual encoding pipelines.

## Architecture

```
UDP Socket (per camera)
    ↓
RTP Packet Parser
    ↓
Jitter Buffer (reorder packets)
    ↓
Frame Assembler (group packets by timestamp)
    ↓
RFC 2435 JPEG Reconstructor
    ↓
JPEG Decoder (to RGB numpy array)
    ↓
Frame Queue (thread-safe)
    ↓
Consumer (visual encoder)
```

## Core Components

### 1. RTP Packet Structure

```python
@dataclass
class RTPPacket:
    """Parsed RTP packet"""
    version: int          # Always 2
    padding: bool
    extension: bool
    marker: bool          # True = last packet of frame
    payload_type: int     # 26 for MJPEG
    sequence: int         # Sequence number (wraps at 65535)
    timestamp: int        # RTP timestamp (90kHz clock)
    ssrc: int            # Synchronization source
    payload: bytes       # JPEG payload (RFC 2435 format)
```

### 2. RFC 2435 JPEG Header

```python
@dataclass
class JPEGHeader:
    """RFC 2435 JPEG-specific header"""
    type_specific: int   # Fragment offset (bits 0-7)
    fragment_offset: int # Offset in bytes (bits 8-31)
    type: int           # JPEG type (0-127)
    q: int              # Quantization table ID
    width: int          # Width / 8
    height: int         # Height / 8
    
    # Parsed from first packet
    has_restart_marker: bool
    restart_interval: int
    restart_count: int
```

### 3. Frame Structure

```python
@dataclass
class Frame:
    """Assembled frame ready for decoding"""
    camera_id: str
    timestamp: int           # RTP timestamp
    sequence_start: int      # First packet sequence number
    sequence_end: int        # Last packet sequence number
    width: int              # Pixels
    height: int             # Pixels
    jpeg_data: bytes        # Complete JPEG file
    packets_received: int
    packets_lost: int
    receive_time: float     # Unix timestamp
```

### 4. Camera Configuration

```python
@dataclass
class CameraConfig:
    """Per-camera configuration"""
    camera_id: str          # Unique identifier
    host: str              # IP address
    port: int              # UDP port
    
    # Expected stream properties
    width: int             # Expected width
    height: int            # Expected height
    fps: int               # Expected frame rate
    
    # Buffer settings
    jitter_buffer_size: int = 30      # Max packets in jitter buffer
    max_frame_age_ms: int = 1000      # Discard incomplete frames older than this
    socket_buffer_size: int = 2097152 # 2MB UDP receive buffer
```

## Implementation Details

### Step 1: RTP Packet Reception

```python
def receive_rtp_packet(sock: socket.socket) -> RTPPacket:
    """
    Receive and parse one RTP packet.
    
    RTP Header (12 bytes):
    0                   1                   2                   3
    0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    |V=2|P|X|  CC   |M|     PT      |       sequence number         |
    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    |                           timestamp                           |
    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    |           synchronization source (SSRC) identifier            |
    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    """
    data, addr = sock.recvfrom(65535)
    
    # Parse RTP header
    version = (data[0] >> 6) & 0x03
    padding = bool(data[0] & 0x20)
    extension = bool(data[0] & 0x10)
    cc = data[0] & 0x0F
    
    marker = bool(data[1] & 0x80)
    payload_type = data[1] & 0x7F
    
    sequence = struct.unpack('!H', data[2:4])[0]
    timestamp = struct.unpack('!I', data[4:8])[0]
    ssrc = struct.unpack('!I', data[8:12])[0]
    
    # Skip CSRC identifiers if present
    header_size = 12 + (cc * 4)
    
    # Skip extension if present
    if extension:
        ext_len = struct.unpack('!H', data[header_size+2:header_size+4])[0]
        header_size += 4 + (ext_len * 4)
    
    # Extract payload
    payload = data[header_size:]
    
    # Remove padding if present
    if padding:
        padding_len = payload[-1]
        payload = payload[:-padding_len]
    
    return RTPPacket(
        version=version,
        padding=padding,
        extension=extension,
        marker=marker,
        payload_type=payload_type,
        sequence=sequence,
        timestamp=timestamp,
        ssrc=ssrc,
        payload=payload
    )
```

### Step 2: RFC 2435 JPEG Payload Parsing

```python
def parse_jpeg_payload(payload: bytes) -> tuple[JPEGHeader, bytes, Optional[bytes]]:
    """
    Parse RFC 2435 JPEG payload.
    
    Returns:
        (header, quantization_tables, scan_data)
    
    JPEG Header (8 bytes minimum):
    0                   1                   2                   3
    0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1 2 3 4 5 6 7 8 9 0 1
    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    | Type-specific |              Fragment Offset                  |
    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    |      Type     |       Q       |     Width     |     Height    |
    +-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+-+
    """
    
    # Parse main header (8 bytes)
    type_specific = payload[0]
    fragment_offset = struct.unpack('!I', b'\x00' + payload[0:3])[0] & 0xFFFFFF
    jpeg_type = payload[4]
    q = payload[5]
    width = payload[6] * 8
    height = payload[7] * 8
    
    offset = 8
    q_tables = None
    
    # Parse Restart Marker header if present (Type >= 64)
    restart_interval = 0
    restart_count = 0
    if jpeg_type >= 64:
        restart_interval = struct.unpack('!H', payload[offset:offset+2])[0]
        restart_count = struct.unpack('!H', payload[offset+2:offset+4])[0]
        offset += 4
    
    # Parse Quantization Table header if present (Q >= 128)
    if q >= 128:
        mbz = payload[offset]
        precision = payload[offset + 1]
        length = struct.unpack('!H', payload[offset+2:offset+4])[0]
        offset += 4
        
        # Extract quantization tables
        q_tables = payload[offset:offset+length]
        offset += length
    
    # Remaining data is scan data
    scan_data = payload[offset:]
    
    header = JPEGHeader(
        type_specific=type_specific,
        fragment_offset=fragment_offset,
        type=jpeg_type,
        q=q,
        width=width,
        height=height,
        has_restart_marker=(jpeg_type >= 64),
        restart_interval=restart_interval,
        restart_count=restart_count
    )
    
    return header, q_tables, scan_data
```

### Step 3: JPEG Reconstruction

```python
def reconstruct_jpeg(
    width: int,
    height: int,
    jpeg_type: int,
    q_tables: bytes,
    scan_data: bytes
) -> bytes:
    """
    Reconstruct complete JPEG from RFC 2435 components.
    
    JPEG structure:
    - SOI  (0xFFD8)
    - APP0 (JFIF marker)
    - DQT  (Quantization tables)
    - SOF0 (Start of Frame)
    - DHT  (Huffman tables)
    - SOS  (Start of Scan)
    - Scan data
    - EOI  (0xFFD9)
    """
    
    jpeg = bytearray()
    
    # 1. SOI marker
    jpeg.extend(b'\xFF\xD8')
    
    # 2. APP0 (JFIF) marker
    jpeg.extend(b'\xFF\xE0')  # APP0 marker
    jpeg.extend(struct.pack('!H', 16))  # Length
    jpeg.extend(b'JFIF\x00')  # Identifier
    jpeg.extend(b'\x01\x01')  # Version 1.1
    jpeg.extend(b'\x00')  # Units (0 = no units)
    jpeg.extend(struct.pack('!HH', 1, 1))  # X/Y density
    jpeg.extend(b'\x00\x00')  # No thumbnail
    
    # 3. DQT markers (Quantization tables)
    # Tables are in q_tables bytes (64 bytes per table)
    num_tables = len(q_tables) // 64
    for i in range(num_tables):
        table_data = q_tables[i*64:(i+1)*64]
        jpeg.extend(b'\xFF\xDB')  # DQT marker
        jpeg.extend(struct.pack('!H', 67))  # Length (2 + 1 + 64)
        jpeg.extend(bytes([i]))  # Table ID
        jpeg.extend(table_data)
    
    # 4. SOF0 marker (Start of Frame - Baseline DCT)
    components = 3 if jpeg_type == 0 else 1  # RGB or Grayscale
    jpeg.extend(b'\xFF\xC0')  # SOF0 marker
    jpeg.extend(struct.pack('!H', 8 + 3 * components))  # Length
    jpeg.extend(b'\x08')  # Precision (8 bits)
    jpeg.extend(struct.pack('!HH', height, width))
    jpeg.extend(bytes([components]))  # Number of components
    
    # Component specifications
    if components == 3:
        # Y component
        jpeg.extend(b'\x01')  # ID
        jpeg.extend(b'\x22')  # Sampling (2x2)
        jpeg.extend(b'\x00')  # Quantization table 0
        # Cb component
        jpeg.extend(b'\x02')  # ID
        jpeg.extend(b'\x11')  # Sampling (1x1)
        jpeg.extend(b'\x01')  # Quantization table 1
        # Cr component
        jpeg.extend(b'\x03')  # ID
        jpeg.extend(b'\x11')  # Sampling (1x1)
        jpeg.extend(b'\x01')  # Quantization table 1
    else:
        # Grayscale
        jpeg.extend(b'\x01\x11\x00')
    
    # 5. DHT markers (Huffman tables - standard tables)
    jpeg.extend(get_standard_huffman_tables())
    
    # 6. SOS marker (Start of Scan)
    jpeg.extend(b'\xFF\xDA')  # SOS marker
    jpeg.extend(struct.pack('!H', 6 + 2 * components))  # Length
    jpeg.extend(bytes([components]))  # Number of components
    
    if components == 3:
        jpeg.extend(b'\x01\x00')  # Y component, DC/AC table 0
        jpeg.extend(b'\x02\x11')  # Cb component, DC/AC table 1
        jpeg.extend(b'\x03\x11')  # Cr component, DC/AC table 1
    else:
        jpeg.extend(b'\x01\x00')  # Grayscale
    
    jpeg.extend(b'\x00\x3F\x00')  # Spectral selection
    
    # 7. Scan data (actual compressed image data)
    jpeg.extend(scan_data)
    
    # 8. EOI marker
    jpeg.extend(b'\xFF\xD9')
    
    return bytes(jpeg)


def get_standard_huffman_tables() -> bytes:
    """
    Return standard JPEG Huffman tables.
    
    These are the default tables defined in ITU-T T.81 (JPEG spec).
    """
    # DC luminance table
    dc_lum_bits = bytes([0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0])
    dc_lum_vals = bytes([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11])
    
    # DC chrominance table
    dc_chrom_bits = bytes([0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0])
    dc_chrom_vals = bytes([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11])
    
    # AC luminance table
    ac_lum_bits = bytes([0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 125])
    ac_lum_vals = bytes([
        0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12,
        0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07,
        0x22, 0x71, 0x14, 0x32, 0x81, 0x91, 0xa1, 0x08,
        0x23, 0x42, 0xb1, 0xc1, 0x15, 0x52, 0xd1, 0xf0,
        0x24, 0x33, 0x62, 0x72, 0x82, 0x09, 0x0a, 0x16,
        0x17, 0x18, 0x19, 0x1a, 0x25, 0x26, 0x27, 0x28,
        0x29, 0x2a, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39,
        0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49,
        0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59,
        0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69,
        0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79,
        0x7a, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89,
        0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98,
        0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7,
        0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6,
        0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3, 0xc4, 0xc5,
        0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4,
        0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2,
        0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea,
        0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
        0xf9, 0xfa
    ])
    
    # AC chrominance table
    ac_chrom_bits = bytes([0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 119])
    ac_chrom_vals = bytes([
        0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21,
        0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71,
        0x13, 0x22, 0x32, 0x81, 0x08, 0x14, 0x42, 0x91,
        0xa1, 0xb1, 0xc1, 0x09, 0x23, 0x33, 0x52, 0xf0,
        0x15, 0x62, 0x72, 0xd1, 0x0a, 0x16, 0x24, 0x34,
        0xe1, 0x25, 0xf1, 0x17, 0x18, 0x19, 0x1a, 0x26,
        0x27, 0x28, 0x29, 0x2a, 0x35, 0x36, 0x37, 0x38,
        0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48,
        0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58,
        0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68,
        0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78,
        0x79, 0x7a, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87,
        0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96,
        0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3, 0xa4, 0xa5,
        0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4,
        0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3,
        0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2,
        0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda,
        0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9,
        0xea, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8,
        0xf9, 0xfa
    ])
    
    tables = bytearray()
    
    # DHT marker + DC luminance
    tables.extend(b'\xFF\xC4')
    tables.extend(struct.pack('!H', 19 + len(dc_lum_vals)))
    tables.extend(b'\x00')  # DC table 0
    tables.extend(dc_lum_bits)
    tables.extend(dc_lum_vals)
    
    # DHT marker + AC luminance
    tables.extend(b'\xFF\xC4')
    tables.extend(struct.pack('!H', 19 + len(ac_lum_vals)))
    tables.extend(b'\x10')  # AC table 0
    tables.extend(ac_lum_bits)
    tables.extend(ac_lum_vals)
    
    # DHT marker + DC chrominance
    tables.extend(b'\xFF\xC4')
    tables.extend(struct.pack('!H', 19 + len(dc_chrom_vals)))
    tables.extend(b'\x01')  # DC table 1
    tables.extend(dc_chrom_bits)
    tables.extend(dc_chrom_vals)
    
    # DHT marker + AC chrominance
    tables.extend(b'\xFF\xC4')
    tables.extend(struct.pack('!H', 19 + len(ac_chrom_vals)))
    tables.extend(b'\x11')  # AC table 1
    tables.extend(ac_chrom_bits)
    tables.extend(ac_chrom_vals)
    
    return bytes(tables)
```

### Step 4: Frame Assembly

```python
class FrameAssembler:
    """Assembles RTP packets into complete frames"""
    
    def __init__(self, camera_id: str, max_age_ms: int = 1000):
        self.camera_id = camera_id
        self.max_age_ms = max_age_ms
        self.packets: dict[int, list[RTPPacket]] = {}  # timestamp -> packets
        self.q_tables_cache: Optional[bytes] = None
        
    def add_packet(self, packet: RTPPacket) -> Optional[Frame]:
        """
        Add RTP packet. Returns complete frame if marker bit is set.
        """
        timestamp = packet.timestamp
        
        # Create packet list for this frame if needed
        if timestamp not in self.packets:
            self.packets[timestamp] = []
        
        self.packets[timestamp].append(packet)
        
        # Check if frame is complete (marker bit set)
        if packet.marker:
            return self._assemble_frame(timestamp)
        
        # Clean up old incomplete frames
        self._cleanup_old_frames()
        
        return None
    
    def _assemble_frame(self, timestamp: int) -> Frame:
        """Assemble complete frame from packets"""
        packets = self.packets.pop(timestamp)
        
        # Sort by sequence number
        packets.sort(key=lambda p: p.sequence)
        
        # Parse first packet to get JPEG header and Q tables
        first_header, q_tables, first_scan = parse_jpeg_payload(packets[0].payload)
        
        # Cache Q tables if present (they usually don't change)
        if q_tables:
            self.q_tables_cache = q_tables
        elif self.q_tables_cache:
            q_tables = self.q_tables_cache
        else:
            raise ValueError("No quantization tables available")
        
        # Collect all scan data
        scan_data = bytearray()
        scan_data.extend(first_scan)
        
        for packet in packets[1:]:
            _, _, scan = parse_jpeg_payload(packet.payload)
            scan_data.extend(scan)
        
        # Reconstruct complete JPEG
        jpeg_data = reconstruct_jpeg(
            width=first_header.width,
            height=first_header.height,
            jpeg_type=first_header.type,
            q_tables=q_tables,
            scan_data=bytes(scan_data)
        )
        
        # Detect packet loss
        seq_numbers = [p.sequence for p in packets]
        expected_count = (seq_numbers[-1] - seq_numbers[0] + 1) % 65536
        packets_lost = expected_count - len(packets)
        
        return Frame(
            camera_id=self.camera_id,
            timestamp=timestamp,
            sequence_start=seq_numbers[0],
            sequence_end=seq_numbers[-1],
            width=first_header.width,
            height=first_header.height,
            jpeg_data=jpeg_data,
            packets_received=len(packets),
            packets_lost=max(0, packets_lost),
            receive_time=time.time()
        )
    
    def _cleanup_old_frames(self):
        """Remove incomplete frames older than max_age_ms"""
        now = time.time() * 1000
        to_remove = []
        
        for timestamp, packets in self.packets.items():
            if packets:
                age = now - (packets[0].timestamp / 90.0)  # Convert 90kHz to ms
                if age > self.max_age_ms:
                    to_remove.append(timestamp)
        
        for timestamp in to_remove:
            del self.packets[timestamp]
```

### Step 5: Main Client Class

```python
import socket
import threading
import queue
from typing import Optional
import numpy as np
import cv2

class MJPEGRTPClient:
    """
    Multi-camera MJPEG-RTP client.
    
    Returns RGB frames as numpy arrays (H, W, 3) uint8.
    """
    
    def __init__(self):
        self._cameras: dict[str, CameraConfig] = {}
        self._receivers: dict[str, threading.Thread] = {}
        self._frame_queues: dict[str, queue.Queue] = {}
        self._running = False
    
    def add_camera(self, config: CameraConfig):
        """Add camera configuration"""
        self._cameras[config.camera_id] = config
        self._frame_queues[config.camera_id] = queue.Queue(maxsize=10)
    
    def start(self):
        """Start receiving from all cameras"""
        if self._running:
            return
        
        self._running = True
        
        for camera_id, config in self._cameras.items():
            thread = threading.Thread(
                target=self._receive_loop,
                args=(config,),
                daemon=True,
                name=f"RTP-{camera_id}"
            )
            thread.start()
            self._receivers[camera_id] = thread
    
    def stop(self):
        """Stop receiving from all cameras"""
        self._running = False
        
        for thread in self._receivers.values():
            thread.join(timeout=2.0)
        
        self._receivers.clear()
    
    def get_frame(self, camera_id: str, timeout: float = 1.0) -> Optional[tuple[np.ndarray, Frame]]:
        """
        Get next RGB frame from camera.
        
        Returns:
            (rgb_array, frame_metadata) or None if timeout
            rgb_array is (H, W, 3) uint8 in RGB format
        """
        try:
            frame = self._frame_queues[camera_id].get(timeout=timeout)
            
            # Decode JPEG to numpy array
            jpg_array = np.frombuffer(frame.jpeg_data, dtype=np.uint8)
            bgr_image = cv2.imdecode(jpg_array, cv2.IMREAD_COLOR)
            
            # Convert BGR to RGB
            rgb_image = cv2.cvtColor(bgr_image, cv2.COLOR_BGR2RGB)
            
            return rgb_image, frame
            
        except queue.Empty:
            return None
    
    def _receive_loop(self, config: CameraConfig):
        """Receiver thread for one camera"""
        # Create UDP socket
        sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        sock.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, config.socket_buffer_size)
        sock.bind(('0.0.0.0', config.port))
        sock.settimeout(1.0)
        
        assembler = FrameAssembler(config.camera_id, config.max_frame_age_ms)
        
        while self._running:
            try:
                # Receive RTP packet
                packet = receive_rtp_packet(sock)
                
                # Add to assembler
                frame = assembler.add_packet(packet)
                
                # If frame is complete, put in queue
                if frame:
                    try:
                        self._frame_queues[config.camera_id].put_nowait(frame)
                    except queue.Full:
                        # Drop oldest frame
                        try:
                            self._frame_queues[config.camera_id].get_nowait()
                            self._frame_queues[config.camera_id].put_nowait(frame)
                        except queue.Empty:
                            pass
                            
            except socket.timeout:
                continue
            except Exception as e:
                print(f"Error on {config.camera_id}: {e}")
                break
        
        sock.close()
```

## Usage for Visual Encoder

```python
# Initialize client
client = MJPEGRTPClient()

# Add cameras
client.add_camera(CameraConfig(
    camera_id="cam1",
    host="192.168.1.100",
    port=17000,
    width=1920,
    height=1080,
    fps=30
))

client.add_camera(CameraConfig(
    camera_id="cam2",
    host="192.168.1.101",
    port=17001,
    width=1280,
    height=720,
    fps=60
))

# Start receiving
client.start()

# Get frames for encoding
while True:
    result = client.get_frame("cam1", timeout=1.0)
    
    if result:
        rgb_frame, metadata = result
        
        # rgb_frame is numpy array (H, W, 3) uint8, ready for encoder
        # Shape: (1080, 1920, 3) for cam1
        # Values: 0-255 RGB
        
        # Send to visual encoder
        encode_frame(rgb_frame)
        
        # Check quality
        if metadata.packets_lost > 0:
            print(f"Frame {metadata.sequence_start} had {metadata.packets_lost} lost packets")

# Cleanup
client.stop()
```

## Dependencies

```toml
[tool.poetry.dependencies]
python = "^3.10"
numpy = "^1.24.0"
opencv-python = "^4.8.0"
```

## File Structure

```
python-client/
├── mjpeg_rtp_client/
│   ├── __init__.py
│   ├── client.py          # MJPEGRTPClient
│   ├── rtp.py             # RTP packet parsing
│   ├── rfc2435.py         # JPEG payload parsing & reconstruction
│   └── frame_assembler.py # Frame assembly
└── tests/
    ├── test_rtp.py
    ├── test_rfc2435.py
    └── test_client.py
```

## Key Points

1. **Thread-safe**: Each camera has dedicated receiver thread
2. **RGB output**: Frames are decoded to RGB numpy arrays ready for encoding
3. **Queue-based**: Non-blocking frame delivery via queues
4. **Packet loss handling**: Detects and reports lost packets
5. **Memory efficient**: Reuses quantization tables between frames
6. **Simple API**: Just `get_frame()` to get RGB numpy array
