import QtQuick
import Quickshell
import qs.Commons
import qs.Commons as Commons
import "gui" as G

ShellRoot {
  id: root
  property int phase: 0
  G.CloudIcon {
    id: barCloud
    monochrome: true
    color: Commons.Color.foreground
  }
  function check(value, message) { if (!value) throw new Error(message) }
  function find(item, name) {
    if (item.objectName === name) return item
    var children = item.children || []
    for (var i = 0; i < children.length; i++) {
      var found = find(children[i], name)
      if (found) return found
    }
    return null
  }
  QtObject {
    id: backend
    property bool ready: true
    property bool connected: true
    property bool powered: true
    property bool powerReady: true
    property bool powerChanging: false
    property bool loginEnabled: false
    property bool loginReady: true
    property bool loginChanging: false
    property bool busy: false
    property string home: "/home/example"
    property string statusText: "Ready"
    property string daemonVersion: "test"
    property var engine: ({state:"running",version:"test"})
    property var appSettings: ({mount_root:"~/Cloud/Stream",local_root:"~/Cloud/Sync",cache_max_size_gb:0,cache_min_free_gb:0,cache_max_age:"720h",sync_transfers:4,sync_checkers:8,max_parallel_syncs:3,notifications:true})
    property var drives: [{name:"Example",label:"Test account",stream:false,folders:[]}]
    property var filters: ({excludes:[],otherRules:0})
    property var statePaths: ({})
    property int backupFiles: 0
    property var lastSetting: null
    property var lastFolder: null
    property bool loginChecked: false
    signal stateUpdated()
    function checkLoginService() { loginChecked = true }
    function setLoginService(on) { loginEnabled = on }
    function setPower(on) { powered = on }
    function setSetting(key, value) {
      lastSetting = {key:key,value:value}
      var copy = Object.assign({}, appSettings)
      copy[key] = value
      appSettings = copy
    }
    function addFolder(remote, path, local, size, done) {
      lastFolder = {remote:remote,path:path,local:local}
      done(true)
    }
  }
  FloatingWindow {
    visible: true
    implicitWidth: 500
    implicitHeight: 720
    color: Commons.Color.popups.background
    Flickable {
      anchors.fill: parent
      contentHeight: settings.implicitHeight
      clip: true
      G.SettingsView {
        id: settings
        width: parent.width
        service: backend
      }
    }
  }
  Timer {
    id: testTimer
    interval: 150
    running: true
    repeat: true
    onTriggered: {
      try {
        if (root.phase === 0) {
          settings.load()
          root.check(backend.loginChecked, "login service status was not requested")
          root.check(root.find(settings,"cache_min_free_gb").value === 0, "zero free-space setting changed")
          root.check(root.find(settings,"cache_max_size_gb").value === 0, "zero cache setting changed")
          root.find(settings,"settingsSection").changed("sync")
          root.check(settings.activeSection === "sync", "section navigation failed")
          root.find(settings,"sync_transfers").modified(8)
          root.check(backend.lastSetting.key === "sync_transfers" && backend.lastSetting.value === 8, "parallelism edit was not sent")
          root.find(settings,"loginServiceSwitch").toggled()
          root.check(backend.loginEnabled, "login service toggle failed")
          root.find(settings,"syncRemotePath").text = "Thinkpad/Documents"
          root.find(settings,"syncLocalPath").text = "~/Documents"
          root.find(settings,"addSyncFolder").clicked()
          root.check(backend.lastFolder.remote === "Example" && backend.lastFolder.path === "Thinkpad/Documents" && backend.lastFolder.local === "~/Documents", "folder mapping was incorrect")
          root.check(!settings.addingFolder, "folder form stayed busy")
          Commons.Color.foreground = "#e8b45a"
          var states = [
            {active: false, busy: false, badge: "Ⅱ"},
            {active: false, busy: false, badge: "⏻"},
            {active: false, busy: false, badge: "!"},
            {active: true, busy: false, badge: ""},
            {active: true, busy: true, badge: ""}
          ]
          for (var i = 0; i < states.length; i++) {
            barCloud.active = states[i].active
            barCloud.busy = states[i].busy
            barCloud.badge = states[i].badge
            root.check(barCloud.children[0].opacity === 1, "bar cloud was dimmed in state " + i)
            root.check(String(barCloud.children[0].color) === "#e8b45a", "bar cloud ignored the theme")
            root.check(String(barCloud.children[1].color) === "#e8b45a", "spinner ignored the theme")
            root.check(String(barCloud.children[2].color) === "#e8b45a", "badge ignored the theme")
          }
          root.phase = 1
        } else {
          root.check(String(settings.foreground) === "#e8b45a", "settings did not follow live theme color")
          settings.openFilters()
          root.check(settings.activeSection === "advanced" && settings.editingFilters, "filter section did not open")
          var preview = Quickshell.env("CLOUDKIT_SETTINGS_PREVIEW") || ""
          if (preview !== "") {
            testTimer.stop()
            settings.grabToImage(function(result) {
              result.saveToFile(preview)
              console.log("SETTINGS_WIDGET_PASS")
              Qt.quit()
            })
          } else {
            console.log("SETTINGS_WIDGET_PASS")
            Qt.quit()
          }
        }
      } catch (e) {
        console.error("SETTINGS_WIDGET_FAIL: " + e)
        Qt.quit()
      }
    }
  }
}
