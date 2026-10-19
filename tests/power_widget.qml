import QtQuick
import Quickshell
import Quickshell.Io
import "gui" as G

ShellRoot {
  id: root
  property int phase: 0
  property int ticks: 0
  Process {
    id: externalOff
    command: [service.cli, "power", "off"]
  }
  G.Service {
    id: service
    pluginDir: Quickshell.env("CLOUDKIT_TEST_PLUGIN")
  }
  Timer {
    interval: 100
    running: true
    repeat: true
    onTriggered: {
      try {
        root.ticks++
        if (root.ticks > 300) throw new Error("timeout phase=" + root.phase + " powered=" + service.powered + " ready=" + service.powerReady + " changing=" + service.powerChanging + " connected=" + service.connected + " engine=" + service.engine.state + " error=" + service.startError)
        if (root.phase === 0 && service.powerReady && !service.powerChanging) {
          if (service.powered || service.connected) throw new Error("disabled profile auto-started")
          root.phase = 1
        } else if (root.phase === 1 && root.ticks >= 10) {
          if (service.connected || service.starting) throw new Error("off state reconnected")
          service.request("touch", {}, function(ok) {
            if (ok) throw new Error("off state accepted a request")
          })
          service.setPower(true)
          root.phase = 2
        } else if (root.phase === 2 && service.ready) {
          service.setPower(false)
          root.phase = 3
        } else if (root.phase === 3 && !service.powerChanging && !service.powered) {
          if (service.connected || service.busy) throw new Error("off state retained activity")
          root.phase = 4
          root.ticks = 0
        } else if (root.phase === 4 && root.ticks >= 15) {
          if (service.connected || service.starting || service.powerChanging) throw new Error("off state restarted")
          service.setPower(true)
          root.phase = 5
        } else if (root.phase === 5 && service.ready) {
          externalOff.running = true
          root.phase = 6
        } else if (root.phase === 6 && !externalOff.running && !service.powerChanging && !service.powered) {
          if (service.connected) throw new Error("external power-off left bridge connected")
          console.log("POWER_WIDGET_PASS")
          Qt.quit()
        }
      } catch (e) {
        console.error("POWER_WIDGET_FAIL: " + e)
        Qt.quit()
      }
    }
  }
}
