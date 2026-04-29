import { buildNvsPartition, type NvsBlob } from '~/utils/nvs'
import type { BoardManifest } from '~/types/firmware'

export interface FlashProgress {
  stage: 'connecting' | 'erasing' | 'flashing' | 'done' | 'error'
  percent: number
  message: string
}

export interface DeviceConfig {
  hostname: string
  board: BoardManifest
  ssid?: string      // optional; written to NVS when non-empty
  password?: string  // optional; paired with ssid
}

export function useSerial() {
  const port = ref<SerialPort | null>(null)
  const logs = ref<string[]>([])
  const monitoring = ref(false)
  let monitorReader: ReadableStreamDefaultReader<Uint8Array> | null = null

  function log(line: string) {
    logs.value.push(line)
    if (logs.value.length > 500) {
      logs.value = logs.value.slice(-500)
    }
  }

  function clearLogs() {
    logs.value = []
  }

  async function requestPort(): Promise<boolean> {
    try {
      port.value = await navigator.serial.requestPort({
        filters: [{ usbVendorId: 0x303a }], // Espressif USB VID
      })
      log('Serial port selected')
      return true
    } catch {
      return false
    }
  }

  async function startMonitor() {
    if (monitoring.value) return

    // After reset, ESP32-S3 USB re-enumerates to a new tty.
    // Need to request the port again from the user.
    log('Select the serial port to monitor (device may have changed port after reboot)')
    const ok = await requestPort()
    if (!ok) {
      log('Monitor: no port selected')
      return
    }

    try {
      await port.value!.open({ baudRate: 115200 })
    } catch (e: any) {
      log(`Monitor: cannot open port — ${e.message}`)
      return
    }

    monitoring.value = true
    log('--- Serial monitor started ---')

    const decoder = new TextDecoder()
    let buffer = ''

    try {
      monitorReader = port.value!.readable!.getReader()
      while (true) {
        const { value, done } = await monitorReader.read()
        if (done) break
        buffer += decoder.decode(value, { stream: true })
        // Flush complete lines
        let nlIdx: number
        while ((nlIdx = buffer.indexOf('\n')) >= 0) {
          const line = buffer.slice(0, nlIdx).replace(/\r$/, '')
          log(line)
          buffer = buffer.slice(nlIdx + 1)
        }
      }
    } catch {
      // Port closed or disconnected
    } finally {
      monitorReader = null
      monitoring.value = false
      log('--- Serial monitor stopped ---')
    }
  }

  async function stopMonitor() {
    if (monitorReader) {
      try { await monitorReader.cancel() } catch { /* ignore */ }
    }
    if (port.value?.readable) {
      try { await port.value.close() } catch { /* ignore */ }
    }
    monitoring.value = false
  }

  async function flashDevice(
    config: DeviceConfig,
    onProgress: (progress: FlashProgress) => void,
  ): Promise<boolean> {
    const { ESPLoader, Transport } = await import('esptool-js')

    const freshPort = await requestPort()
    if (!freshPort) {
      onProgress({ stage: 'error', percent: 0, message: 'No serial port selected' })
      return false
    }

    clearLogs()
    log('Starting flash process...')
    onProgress({ stage: 'connecting', percent: 0, message: 'Connecting to ESP32...' })

    const terminal = {
      clean: () => { clearLogs() },
      writeLine: (data: string) => { log(data) },
      write: (data: string) => {
        const last = logs.value[logs.value.length - 1]
        if (logs.value.length > 0 && last && !last.endsWith('\n')) {
          logs.value[logs.value.length - 1] = last + data
        } else {
          log(data)
        }
      },
    }

    try {
      // If device is in application mode (PID 0x4001), reboot it into bootloader
      const info = port.value!.getInfo()
      if (info.usbProductId === 0x4001) {
        log('Device is in application mode — rebooting into bootloader...')
        onProgress({ stage: 'connecting', percent: 2, message: 'Rebooting into bootloader...' })
        if (!port.value!.readable) {
          await port.value!.open({ baudRate: 115200 })
        }
        const writer = port.value!.writable!.getWriter()
        const enc = new TextEncoder()
        // Ctrl+C to interrupt running code, then enter bootloader
        await writer.write(enc.encode('\r\x03\x03'))
        await new Promise(r => setTimeout(r, 300))
        await writer.write(enc.encode('\r\nimport machine; machine.bootloader()\r\n'))
        writer.releaseLock()
        await new Promise(r => setTimeout(r, 500))
        try { await port.value!.close() } catch { /* device already gone */ }

        // Device re-enumerates as bootloader — user must select the new port
        log('Device is rebooting — select the new "USB JTAG/serial debug unit" port')
        const ok = await requestPort()
        if (!ok) {
          onProgress({ stage: 'error', percent: 0, message: 'No bootloader port selected' })
          return false
        }
      }

      // Patch setSignals for USB-OTG/CDC boards
      const rawSetSignals = port.value!.setSignals.bind(port.value!)
      let signalWarned = false
      port.value!.setSignals = async (signals: SerialOutputSignals) => {
        try {
          await rawSetSignals(signals)
        } catch {
          if (!signalWarned) {
            log('USB-CDC: control signals not supported')
            signalWarned = true
          }
        }
      }

      const transport = new Transport(port.value!)
      const loader = new ESPLoader({
        transport,
        baudrate: 921600,
        terminal,
      })

      log('Connecting to bootloader...')
      await loader.main()
      // --- Chip-vs-board guard ---
      const detectedChip = loader.chip?.CHIP_NAME || 'ESP32'
      log(`Chip detected: ${detectedChip}`)
      onProgress({ stage: 'connecting', percent: 10, message: `Chip: ${detectedChip}` })
      if (detectedChip !== config.board.chip) {
        throw new Error(
          `Selected ${config.board.name} (${config.board.chip}) but detected ${detectedChip}. `
          + `Plug in the correct board or change selection.`,
        )
      }

      // --- Download merged firmware image ---
      onProgress({ stage: 'flashing', percent: 15, message: 'Downloading firmware...' })
      const base = useRuntimeConfig().app.baseURL
      const fwResponse = await fetch(base + 'firmware/' + config.board.image)
      if (!fwResponse.ok) {
        throw new Error(
          `Firmware ${config.board.image} missing (HTTP ${fwResponse.status}) — `
          + `rebuild via scripts/build-rust-firmware.sh`,
        )
      }
      const fwData = new Uint8Array(await fwResponse.arrayBuffer())
      log(`Firmware: ${fwData.length} bytes`)

      // --- Build NVS partition ---
      const nvsEntries: NvsBlob[] = [
        { namespace: 'device', key: 'hostname', value: config.hostname },
      ]
      if (config.ssid) {
        nvsEntries.push(
          { namespace: 'wifi', key: 'ssid', value: config.ssid },
          { namespace: 'wifi', key: 'password', value: config.password ?? '' },
        )
        log(`Building NVS: hostname=${config.hostname}, WiFi=${config.ssid}`)
      } else {
        log(`Building NVS: hostname=${config.hostname} (no WiFi creds)`)
      }
      const nvsData = buildNvsPartition(nvsEntries)

      // --- Flash merged image + NVS ---
      log('Flashing firmware + NVS...')
      onProgress({ stage: 'flashing', percent: 25, message: 'Flashing...' })
      await loader.writeFlash({
        fileArray: [
          { data: fwData,  address: 0x0 },     // bootloader + partition table + app (chip-correct internal layout)
          { data: nvsData, address: 0x9000 },  // NVS partition (hostname + WiFi creds if applicable)
        ],
        flashSize: 'keep',
        flashMode: 'keep',
        flashFreq: 'keep',
        eraseAll: true,
        compress: true,
        reportProgress: (_fileIndex: number, written: number, total: number) => {
          const pct = 25 + Math.round((written / total) * 70)
          onProgress({ stage: 'flashing', percent: pct, message: `${written}/${total} bytes` })
        },
      })
      // Watchdog reset for ESP32-S3 native USB (no DTR/RTS hardware reset line).
      // Ported from Python esptool: esptool/targets/esp32s3.py watchdog_reset()
      // Arms the RTC watchdog to fire a full system reset after ~100ms.
      log('Resetting device via watchdog...')
      try {
        const WDT_WPROTECT = 0x600080B0
        const WDT_CONFIG0  = 0x60008098
        const WDT_CONFIG1  = 0x6000809C
        const WDT_WKEY     = 0x50D83AA1
        await loader.writeReg(WDT_WPROTECT, WDT_WKEY)                          // unlock
        await loader.writeReg(WDT_CONFIG1, 2000)                                // timeout ms
        await loader.writeReg(WDT_CONFIG0, (1 << 31) | (5 << 28) | (1 << 8) | 2)  // enable + sys reset
        await loader.writeReg(WDT_WPROTECT, 0)                                 // re-lock
        await new Promise(r => setTimeout(r, 500))
        log('Watchdog reset triggered — device is rebooting.')
      } catch {
        log('Watchdog reset failed — press RST button on the device.')
      }

      onProgress({ stage: 'done', percent: 100, message: 'Flash complete! Device is rebooting.' })
      await transport.disconnect()

      return true
    } catch (e: any) {
      log(`ERROR: ${e.message || e}`)
      onProgress({ stage: 'error', percent: 0, message: e.message || 'Flash failed' })
      return false
    }
  }

  return {
    logs: readonly(logs),
    monitoring: readonly(monitoring),
    flashDevice,
    startMonitor,
    stopMonitor,
    clearLogs,
  }
}
