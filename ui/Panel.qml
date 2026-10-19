import QtQuick
import Quickshell
import Quickshell.Io
import qs.Commons
import qs.Commons as Commons
import qs.Ui

Panel {
  id: root
  moduleName: "sabb4th.cloudkit"
  ipcTarget: "sabb4th.cloudkit"
  manageIpc: false

  readonly property color foreground: bar ? bar.foreground : Commons.Color.foreground
  readonly property color urgent: bar ? bar.urgent : Commons.Color.urgent
  readonly property string fontFamily: bar ? bar.fontFamily : Style.font.family
  // The plugin's root folder, wherever it was installed (copy, symlink or git clone)
  readonly property string pluginDir: decodeURIComponent(String(Qt.resolvedUrl("..")).replace(/^file:\/\//, "")).replace(/\/$/, "")

  readonly property color statusColor: service.statusState === "error" ? root.urgent : root.foreground

  readonly property string cloudState: {
    if (!service.powered) return "off"
    if (service.attention || service.actionFailed || service.startError !== "") return "error"
    if (!service.connected || !service.online) return "offline"
    if (!service.ready) return "pending"
    if (service.summary.syncing > 0) return "syncing"
    if (service.summary.uploads > 0) return "uploading"
    if (service.transfers.count > 0) return "syncing"
    var paused = false
    var pending = false
    for (var i = 0; i < service.drives.length; i++) {
      var folders = service.drives[i].folders || []
      for (var j = 0; j < folders.length; j++) {
        paused = paused || folders[j].paused === true
        pending = pending || ["pending", "changes", "waiting"].indexOf(folders[j].state) >= 0
      }
    }
    if (paused) return "paused"
    if (pending) return "pending"
    return service.statusState === "online" ? "synced" : "idle"
  }

  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight

  onOpenedChanged: if (opened) {
    service.touch()
    content.resetScroll()
    Qt.callLater(function() { keyCatcher.forceActiveFocus() })
  }

  Service {
    id: service
    settings: root.settings
    pluginDir: root.pluginDir
  }

  IpcHandler {
    target: root.ipcTarget
    function open(): void { root.open() }
    function close(): void { root.close() }
    function toggle(): void { root.toggle() }
    function add(): void {
      root.open()
      content.showView("add")
    }
    function settings(): void {
      root.open()
      content.showView("settings")
    }
    function syncAll(): string { service.syncNow(""); return "ok" }
    function streamAll(): string { service.streamAll(true); return "ok" }
    function status(): string {
      return JSON.stringify({ connected: service.connected, summary: service.summary, text: service.statusText })
    }
  }

  BarIconButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    tooltipText: ""

    iconComponent: Component {
      Item {
        anchors.fill: parent

        CloudIcon {
          anchors.centerIn: parent
          iconSize: Style.bar.iconFont
          color: root.barForeground
          fontFamily: root.fontFamily
          monochrome: true
          active: root.cloudState === "synced"
          busy: root.cloudState === "syncing"
          badge: ({ off: "⏻", error: "!", offline: "×", paused: "Ⅱ", pending: "…", uploading: "↑" })[root.cloudState] || ""
          attention: false
        }
      }
    }

    onPressed: function(buttonCode) {
      if (buttonCode === Qt.RightButton) service.syncNow("")
      else if (buttonCode === Qt.MiddleButton) service.streamAll(!service.allStreaming)
      else root.toggle()
    }
  }

  KeyboardPanel {
    id: panel
    anchorItem: button
    owner: root
    bar: root.bar
    open: root.opened
    focusTarget: keyCatcher
    contentWidth: panel.fittedContentWidth(Style.space(430))
    contentHeight: panel.fittedContentHeight(content.preferredHeight, Style.space(620))

    PanelKeyCatcher {
      id: keyCatcher
      anchors.fill: parent
      onCloseRequested: {
        if (content.currentView === "log") content.showView("settings")
        else if (content.currentView !== "drives") content.showView("drives")
        else root.close()
      }
      onTabRequested: function(direction) { root.switchPanel(direction) }
      onMoveRequested: function(dx, dy) { if (dy !== 0) content.scrollBy(dy) }
      onTextKey: function(t) {
        if (content.currentView !== "drives") return
        var k = t.toLowerCase()
        if (k === "a") content.showView("add")
        else if (k === "s") content.showView("settings")
        else if (k === "r") service.syncNow("")
        else if (k === "m") service.streamAll(!service.allStreaming)
        else if (k === "o") service.openPath(service.cloudRoot)
      }

      PanelContent {
        id: content
        anchors.fill: parent
        service: service
        foreground: root.foreground
        urgent: root.urgent
        fontFamily: root.fontFamily
        statusColor: root.statusColor
        // Through the fade-out too, while the window is still mapped
        animate: panel.visible && service.powered
      }
    }
  }
}
