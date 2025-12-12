# DeepCool CH170 Digital Display Controller

A Windows application that updates the DeepCool CH170 Digital display with real-time system monitoring data from HWiNFO. The display automatically cycles through different monitoring modes showing CPU and GPU statistics.

## Features

- **Real-time Monitoring**: Displays live system metrics on the CH170 Digital display
- **Multiple Display Modes**: Automatically rotates between three display modes:
  - CPU Frequency mode (CPU temp, power, usage, frequency, cooler RPM)
  - GPU mode (GPU temp, power, usage, frequency)
  - CPU Fan mode (CPU temp, power, usage, frequency, cooler RPM)
- **HWiNFO Integration**: Reads sensor data directly from HWiNFO's shared memory
- **Auto-reconnection**: Automatically handles device disconnections and reconnects

## Requirements

1. **HWiNFO64**: Must be running in the background with shared memory enabled
   - Download from: https://www.hwinfo.com/
   - Enable "Shared Memory Support" in HWiNFO settings
2. **DeepCool CH170 Digital Display**: Must be connected via USB
3. **Windows OS**: This application uses Windows-specific APIs

## Installation

### From Source

1. Install [Mise](https://mise.jdx.dev/getting-started.html) (if not already installed)
2. Clone this repository:
   ```bash
   git clone https://github.com/saanuregh/deepcool-ch170.git
   cd deepcool-ch170
   mise install
   ```
3. Build the release version:
   ```bash
   mise run build
   ```
4. The executable will be located at `target\release\deepcool-ch170.exe`


## Usage

1. Start HWiNFO64 with shared memory support enabled
2. Ensure your DeepCool CH170 Digital display is connected via USB
3. Run the executable:
   ```bash
   deepcool-ch170.exe
   ```

The application will:

- Automatically detect and connect to HWiNFO's shared memory
- Connect to the CH170 display device (VID: 0x363B, PID: 0x0013)
- Begin updating the display with sensor data
- Cycle through display modes every 5 refresh cycles (configurable in code)

To stop the application close in task manager.

## Configuration

### Display Mode Cycle Duration

The time each mode is displayed is controlled by the `REFRESH_CYCLES_PER_MODE` constant in `src/main.rs`:

```rust
const REFRESH_CYCLES_PER_MODE: u32 = 5;
```

Actual duration = `REFRESH_CYCLES_PER_MODE × HWiNFO polling period`

### Temperature Units

Temperature units can be changed in `src/ch_170.rs`:

```rust
const TEMPERATURE_UNIT_CELSIUS: bool = false;  // Set to true for Celsius
```

### Sensor Mapping

The application automatically maps HWiNFO sensors by reading the shared memory. You may need to adjust sensor names/labels in HWiNFO to match what the application expects, or modify the sensor reading logic in `src/sensor_reader.rs`.

## Technical Details

### Architecture

- **Language**: Rust 2024 Edition
- **HID Communication**: Uses `hidapi` for USB HID communication with the display
- **Sensor Reading**: Reads from HWiNFO's shared memory using Windows APIs
- **Logging**: Structured logging with `tracing` crate

### Display Protocol

The CH170 display uses a custom HID protocol:

- Report ID: 16 (0x10)
- Payload size: 64 bytes
- Includes checksumming for data integrity
- Supports temperature, power, usage, frequency, and fan speed metrics

### Project Structure

```
deepcool-ch170/
├── src/
│   ├── main.rs           # Application entry point and main loop
│   ├── ch_170.rs         # CH170 display communication and protocol
│   ├── sensor_reader.rs  # HWiNFO shared memory reader
│   └── helpers.rs        # Utility functions (retry logic, etc.)
├── Cargo.toml            # Rust project configuration
├── LICENSE               # MIT License
└── README.md             # This file
```

### Dependencies

- `hidapi` - USB HID device communication
- `windows` - Windows API bindings
- `zerocopy` - Zero-copy parsing and serialization
- `tracing` - Structured logging
- `anyhow` - Error handling
- `signal-hook` - Signal handling for graceful shutdown

## Troubleshooting

### "Failed to open HID device"

- Ensure the CH170 display is connected via USB
- Check Device Manager for the device (should appear under "Human Interface Devices")
- Try unplugging and replugging the device

### "Failed to initialize sensor reader"

- Make sure HWiNFO64 is running
- Enable "Shared Memory Support" in HWiNFO settings (Sensors → Settings → Shared Memory Support)
- Wait a few seconds after starting HWiNFO before running this application

### Display shows incorrect values

- Verify sensor names in HWiNFO match what the application expects
- Check HWiNFO's sensor readings to ensure they're updating
- Review application logs for sensor reading errors

### No display updates

- Check that HWiNFO is actively updating sensor readings
- Ensure the polling period in HWiNFO is reasonable (recommended: 2000ms)
- Look for errors in the application logs

## Development

### Running in Debug Mode

Debug builds show a console window with logging output:

```bash
mise run dev
```

### Running Tests

```bash
mise run test
```

### Building for Release

```bash
mise run build
```

Release builds are optimized (LTO enabled) and run without a console window.


## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.


## Acknowledgments

- [Nortank12](https://github.com/Nortank12) for the original Linux implementation
