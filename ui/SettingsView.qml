import QtQuick
import QtQuick.Layouts
import qs.Commons
import qs.Commons as Commons
import qs.Ui
import "Model.js" as Model

Item {
  id: root

  property var service: null
  property color foreground: Commons.Color.foreground
  property string fontFamily: Style.font.family

  signal back()
  signal logRequested()
  signal browseRequested(string remote)
  signal addAccountRequested()
  property bool addingFolder: false
  property bool loaded: false
  property string activeSection: "general"
  property string selectedRemote: ""
  readonly property var driveOptions: service ? service.drives.map(function(d) { return { value: d.name, label: d.label || d.name } }) : []

  readonly property color dim: Qt.darker(foreground, 1.6)
  readonly property var s: service ? service.appSettings : ({})
  onDriveOptionsChanged: {
    if (!driveOptions.some(function(d) { return d.value === root.selectedRemote }))
      selectedRemote = driveOptions.length > 0 ? driveOptions[0].value : ""
  }
  property bool editingFilters: false
  readonly property var globalExcludes: service ? (service.filters.excludes || []) : []
  // Folders with their own list, which the global one doesn't reach
  readonly property var ownListFolders: {
    var out = []
    var drives = service ? service.drives : []
    for (var i = 0; i < drives.length; i++)
      for (var j = 0; j < (drives[i].folders || []).length; j++)
        if ((drives[i].folders[j].excludes || []).length > 0) out.push(drives[i].folders[j].name)
    return out
  }

  function openFilters() {
    activeSection = "advanced"
    filterEditor.reset(globalExcludes, globalExcludes)
    editingFilters = true
  }

  implicitHeight: col.implicitHeight

  function load() {
    mountRootField.text = String(s.mount_root || "")
    localRootField.text = String(s.local_root || "")
    cacheAgeField.text = String(s.cache_max_age || "720h")
    if (!driveOptions.some(function(d) { return d.value === root.selectedRemote }))
      selectedRemote = driveOptions.length > 0 ? driveOptions[0].value : ""
    loaded = service && service.ready
    editingFilters = false
    if (service) service.checkLoginService()
  }

  Connections {
    target: root.service
    function onStateUpdated() { if (root.visible && !root.loaded && root.service.ready) root.load() }
  }

  component Card: BorderSurface {
    default property alias contents: cardCol.data
    Layout.fillWidth: true
    implicitHeight: cardCol.implicitHeight + Style.space(20)
    radius: Style.cornerRadius
    color: Commons.Util.alpha(root.foreground, 0.03)
    borderSpec: Border.controlSpec("normal", root.foreground, Commons.Color.accent)
    ColumnLayout {
      id: cardCol
      anchors { left: parent.left; right: parent.right; top: parent.top; margins: Style.space(10) }
      spacing: Style.space(10)
    }
  }

  component NumericSetting: RowLayout {
    property string title: ""
    property string hint: ""
    property string settingKey: ""
    property int minimum: 0
    property int maximum: 100
    property int fallback: 0
    id: settingRow
    Layout.fillWidth: true
    spacing: Style.space(8)
    ColumnLayout {
      Layout.fillWidth: true
      spacing: 0
      Label { text: settingRow.title; wrapMode: Text.WordWrap; Layout.fillWidth: true }
      Hint { text: settingRow.hint }
    }
    NumberField {
      objectName: settingRow.settingKey
      value: root.s[settingRow.settingKey] === undefined ? settingRow.fallback : Number(root.s[settingRow.settingKey])
      from: settingRow.minimum
      to: settingRow.maximum
      foreground: root.foreground
      fontFamily: root.fontFamily
      onModified: function(v) { root.service.setSetting(settingRow.settingKey, v) }
    }
  }

  component Label: Text {
    textFormat: Text.PlainText
    font.family: root.fontFamily
    font.pixelSize: Style.font.caption
    font.bold: true
    color: root.foreground
  }

  component Hint: Text {
    textFormat: Text.PlainText
    Layout.fillWidth: true
    font.family: root.fontFamily
    font.pixelSize: Style.font.caption - Style.space(2)
    color: root.dim
    wrapMode: Text.WordWrap
  }

  // An engine action: icon over a short label, so four of them share one row at panel width.
  // Same API and state colors as the kit's Button.
  component EngineAction: BorderSurface {
    id: tile

    property string text: ""
    property string iconText: ""
    property string tooltipText: ""
    // Its panel is open below the row
    property bool active: false
    signal clicked()

    Layout.fillWidth: true
    Layout.preferredWidth: 1
    implicitWidth: tileCol.implicitWidth + Style.space(12)
    implicitHeight: tileCol.implicitHeight + Style.space(16)
    radius: Style.cornerRadius
    opacity: enabled ? 1 : 0.45
    color: tileMouse.pressed ? Style.pressedFillFor(root.foreground, Commons.Color.accent)
         : tileMouse.containsMouse ? Style.hoverFillFor(root.foreground, Commons.Color.accent)
         : tile.active ? Commons.Util.alpha(Commons.Color.accent, 0.10) : "transparent"
    borderSpec: Border.controlSpec(tile.active ? "selected" : (tileMouse.containsMouse ? "hover-cursor" : "normal"), root.foreground, Commons.Color.accent)

    Column {
      id: tileCol
      anchors.centerIn: parent
      spacing: Style.space(4)

      Text {
        anchors.horizontalCenter: parent.horizontalCenter
        text: tile.iconText
        font.family: root.fontFamily
        font.pixelSize: Style.font.icon
        color: tileMouse.containsMouse || tile.active ? Commons.Color.accent : root.foreground
      }

      Text {
        anchors.horizontalCenter: parent.horizontalCenter
        textFormat: Text.PlainText
        text: tile.text
        font.family: root.fontFamily
        font.pixelSize: Style.font.caption - Style.space(1)
        color: root.foreground
      }
    }

    MouseArea {
      id: tileMouse
      anchors.fill: parent
      enabled: tile.enabled
      hoverEnabled: true
      cursorShape: Qt.PointingHandCursor
      onClicked: tile.clicked()
    }

    PanelToolTip {
      visible: tileMouse.containsMouse && tile.tooltipText !== ""
      text: tile.tooltipText
      fontFamily: root.fontFamily
    }
  }

  ColumnLayout {
    id: col
    width: parent.width
    spacing: Style.space(12)

    RowLayout {
      Layout.fillWidth: true
      spacing: Style.space(8)

      Button {
        iconText: "󰁍"
        text: "Voltar"
        fontFamily: root.fontFamily
        fontSize: Style.font.caption
        foreground: root.foreground
        bordered: true
        onClicked: root.back()
      }

      Text {
        Layout.fillWidth: true
        text: "Configurações"
        font.family: root.fontFamily
        font.pixelSize: Style.font.bodySmall
        font.bold: true
        color: root.foreground
      }
    }

    Dropdown {
      objectName: "settingsSection"
      Layout.fillWidth: true
      showLabel: false
      value: root.activeSection
      foreground: root.foreground
      fontFamily: root.fontFamily
      options: [
        { value: "general", label: "Geral" },
        { value: "folders", label: "Contas e pastas" },
        { value: "sync", label: "Sincronização e desempenho" },
        { value: "cache", label: "Cache do streaming" },
        { value: "paths", label: "Caminhos e proteção local" },
        { value: "advanced", label: "Ferramentas, filtros e logs" }
      ]
      onChanged: function(v) { root.activeSection = v }
    }

    Card {
      visible: root.activeSection === "general"
      Label { text: "Funcionamento" }
      RowLayout {
        Layout.fillWidth: true
        ColumnLayout {
          Layout.fillWidth: true
          Label { text: "Chave geral" }
          Hint { text: "Desliga sincronização, streaming e processos em segundo plano. O botão permanece na barra para religar." }
        }
        ToggleSwitch {
          checked: root.service.powered
          enabled: root.service.powerReady && !root.service.powerChanging
          foreground: root.foreground
          onToggled: root.service.setPower(!root.service.powered)
        }
      }
      RowLayout {
        Layout.fillWidth: true
        ColumnLayout {
          Layout.fillWidth: true
          Label { text: "Serviço no login" }
          Hint { text: "Inicia o serviço mesmo sem o widget na barra. A chave geral desligada continua sendo respeitada." }
        }
        ToggleSwitch {
          objectName: "loginServiceSwitch"
          checked: root.service.loginEnabled
          enabled: root.service.loginReady && !root.service.loginChanging
          foreground: root.foreground
          onToggled: root.service.setLoginService(!root.service.loginEnabled)
        }
      }
      RowLayout {
        Layout.fillWidth: true
        ColumnLayout {
          Layout.fillWidth: true
          Label { text: "Notificações" }
          Hint { text: "Avisos de conflitos e problemas que precisam de atenção." }
        }
        ToggleSwitch {
          checked: root.s.notifications === true
          enabled: root.service.ready
          foreground: root.foreground
          onToggled: root.service.setSetting("notifications", !(root.s.notifications === true))
        }
      }
      Hint {
        visible: !root.service.ready
        text: root.service.powerChanging ? root.service.statusText : "Ligue a chave geral para editar as configurações de sincronização e cache."
      }
    }

    Card {
      visible: root.activeSection === "folders"
      enabled: root.service.ready
      opacity: enabled ? 1 : 0.5
      Label { text: "Adicionar pasta sincronizada" }
      Hint { text: "Escolha os dois caminhos diretamente. A sincronização é bidirecional: alterações e exclusões podem chegar aos dois lados. Não é backup somente de envio." }
      Dropdown {
        id: driveChoice
        objectName: "syncDriveChoice"
        Layout.fillWidth: true
        label: "Conta"
        value: root.selectedRemote
        options: root.driveOptions
        foreground: root.foreground
        fontFamily: root.fontFamily
        onChanged: function(v) { root.selectedRemote = v }
      }
      Label { text: "Pasta dentro da nuvem" }
      TextField {
        id: remotePathField
        objectName: "syncRemotePath"
        Layout.fillWidth: true
        placeholderText: "Thinkpad/Documents"
      }
      Label { text: "Pasta neste computador" }
      TextField {
        id: syncLocalField
        objectName: "syncLocalPath"
        Layout.fillWidth: true
        placeholderText: "~/Documents"
      }
      Hint { text: "O caminho da nuvem é relativo à conta selecionada. Os arquivos existentes nos dois lados serão combinados na primeira sincronização." }
      RowLayout {
        Layout.fillWidth: true
        Button {
          objectName: "addSyncFolder"
          text: root.addingFolder ? "Adicionando…" : "Adicionar pasta"
          enabled: !root.addingFolder && root.selectedRemote !== "" && remotePathField.text.trim() !== "" && syncLocalField.text.trim() !== ""
          foreground: root.foreground
          fontFamily: root.fontFamily
          bordered: true
          onClicked: {
            root.addingFolder = true
            root.service.addFolder(root.selectedRemote, remotePathField.text.trim(), syncLocalField.text.trim(), 0, function(ok) {
              root.addingFolder = false
              if (ok) {
                remotePathField.text = ""
                syncLocalField.text = ""
              }
            })
          }
        }
        Button {
          text: "Navegar na nuvem"
          enabled: root.selectedRemote !== ""
          foreground: root.foreground
          fontFamily: root.fontFamily
          onClicked: root.browseRequested(root.selectedRemote)
        }
      }
      Button {
        visible: root.driveOptions.length === 0
        text: "Conectar uma conta"
        foreground: root.foreground
        fontFamily: root.fontFamily
        onClicked: root.addAccountRequested()
      }
    }

    Card {
      visible: root.activeSection === "folders"
      enabled: root.service.ready
      opacity: enabled ? 1 : 0.5
      Label { text: "Contas e pastas existentes" }
      Hint { text: "A chave de cada conta controla o streaming. Use os controles de cada pasta para pausar, sincronizar, renomear, filtrar ou remover a sincronização." }
      Repeater {
        model: root.service.drives
        delegate: ColumnLayout {
          id: account
          required property var modelData
          Layout.fillWidth: true
          spacing: Style.space(6)
          RowLayout {
            Layout.fillWidth: true
            Label { Layout.fillWidth: true; text: account.modelData.label || account.modelData.name }
            Text { text: "Streaming"; color: root.foreground; font.family: root.fontFamily; font.pixelSize: Style.font.caption }
            ToggleSwitch {
              checked: account.modelData.stream === true
              foreground: root.foreground
              onToggled: root.service.setStream(account.modelData.name, !(account.modelData.stream === true), false)
            }
          }
          Repeater {
            model: account.modelData.folders || []
            delegate: ColumnLayout {
              id: folderEntry
              required property var modelData
              Layout.fillWidth: true
              spacing: Style.space(2)
              Hint {
                text: account.modelData.name + ":" + folderEntry.modelData.path + " ↔ " + Model.tildify(folderEntry.modelData.local, root.service.home)
              }
              FolderRow {
                Layout.fillWidth: true
                folder: folderEntry.modelData
                service: root.service
                foreground: root.foreground
                fontFamily: root.fontFamily
                home: root.service.home
                animate: root.visible && root.service.powered
              }
            }
          }
        }
      }
    }

    Card {
      visible: root.activeSection === "sync"
      enabled: root.service.ready
      opacity: enabled ? 1 : 0.5
      Label { text: "Sincronização e desempenho" }
      NumericSetting {
        title: "Verificar a nuvem a cada (min)"
        hint: "Mudanças locais são detectadas em segundos; este intervalo procura mudanças na nuvem."
        settingKey: "sync_interval_min"; minimum: 1; maximum: 240; fallback: 5
      }
      NumericSetting {
        title: "Transferências por pasta"
        hint: "Padrão: 4. Mais transferências podem ajudar com arquivos pequenos, mas usam mais recursos e chamadas ao provedor."
        settingKey: "sync_transfers"; minimum: 1; maximum: 32; fallback: 4
      }
      NumericSetting {
        title: "Verificações por pasta"
        hint: "Quantidade de arquivos comparados em paralelo. Padrão: 8."
        settingKey: "sync_checkers"; minimum: 1; maximum: 64; fallback: 8
      }
      NumericSetting {
        title: "Pastas simultâneas"
        hint: "Padrão: 3. Reduza para economizar memória, CPU e conexões."
        settingKey: "max_parallel_syncs"; minimum: 1; maximum: 8; fallback: 3
      }
      Hint { text: "As opções de paralelismo valem para as próximas sincronizações. Transferências já iniciadas continuam com os valores anteriores." }
    }

    Card {
      visible: root.activeSection === "cache"
      enabled: root.service.ready
      opacity: enabled ? 1 : 0.5
      Label { text: "Cache do streaming" }
      NumericSetting {
        title: "Tamanho máximo (GB)"
        hint: "Arquivos acessados ficam no cache. 0 desativa o limite de tamanho; não é um limite rígido durante o uso."
        settingKey: "cache_max_size_gb"; minimum: 0; maximum: 4000; fallback: 20
      }
      NumericSetting {
        title: "Espaço livre mínimo (GB)"
        hint: "O rclone tenta liberar cache para manter este espaço disponível. 0 desativa esta reserva."
        settingKey: "cache_min_free_gb"; minimum: 0; maximum: 4000; fallback: 5
      }
      Label { text: "Idade máxima do cache" }
      RowLayout {
        Layout.fillWidth: true
        TextField {
          id: cacheAgeField
          objectName: "cacheMaxAge"
          Layout.fillWidth: true
          placeholderText: "720h"
          onAccepted: root.service.setSetting("cache_max_age", text.trim())
        }
        Button {
          text: "Salvar"
          foreground: root.foreground
          fontFamily: root.fontFamily
          bordered: true
          onClicked: root.service.setSetting("cache_max_age", cacheAgeField.text.trim())
        }
      }
      Hint { text: "Exemplos: 24h, 7d ou 720h. As configurações de cache serão usadas na próxima montagem da conta; a sincronização de pastas não usa esse cache." }
    }

    Card {
      visible: root.activeSection === "paths"
      enabled: root.service.ready
      opacity: enabled ? 1 : 0.5
      Label { text: "Caminhos e proteção local" }
      Label { text: "Montar contas em" }
      RowLayout {
        Layout.fillWidth: true
        TextField {
          id: mountRootField
          objectName: "mountRootPath"
          Layout.fillWidth: true
          placeholderText: "~/Cloud/Stream"
          onAccepted: root.service.setSetting("mount_root", text.trim())
        }
        Button {
          text: "Salvar"; foreground: root.foreground; fontFamily: root.fontFamily; bordered: true
          onClicked: root.service.setSetting("mount_root", mountRootField.text.trim())
        }
      }
      Hint { text: "Cada conta sem caminho próprio é montada em uma subpasta deste diretório. Mudar o caminho remonta essas contas." }
      Label { text: "Pasta local padrão para novas sincronizações" }
      RowLayout {
        Layout.fillWidth: true
        TextField {
          id: localRootField
          objectName: "localRootPath"
          Layout.fillWidth: true
          placeholderText: "~/Cloud/Sync"
          onAccepted: root.service.setSetting("local_root", text.trim())
        }
        Button {
          text: "Salvar"; foreground: root.foreground; fontFamily: root.fontFamily; bordered: true
          onClicked: root.service.setSetting("local_root", localRootField.text.trim())
        }
      }
      Hint { text: "É apenas o local sugerido para novas pastas: não move pastas existentes nem escolhe o destino na nuvem." }
      NumericSetting {
        title: "Guardar versões locais anteriores (dias)"
        hint: "Retenção de arquivos locais que a sincronização substituiu ou apagou. Não é um histórico completo da nuvem."
        settingKey: "backup_days"; minimum: 1; maximum: 365; fallback: 30
      }
    }

    BorderSurface {
      visible: root.activeSection === "advanced"
      Layout.fillWidth: true
      implicitHeight: engCol.implicitHeight + Style.space(20)
      radius: Style.cornerRadius
      color: Commons.Util.alpha(root.foreground, 0.03)
      borderSpec: Border.controlSpec("normal", root.foreground, Commons.Color.accent)

      ColumnLayout {
        id: engCol
        anchors {
          fill: parent
          margins: Style.space(10)
        }
        spacing: Style.space(6)

        Label { text: "Motor e ferramentas" }

        Text {
          Layout.fillWidth: true
          textFormat: Text.PlainText
          text: root.service.connected
                ? ("CloudKit " + root.service.daemonVersion + " · " + (root.service.engine.version || "rclone") + " · " + root.service.engine.state)
                : "Background service not connected"
          font.family: root.fontFamily
          font.pixelSize: Style.font.caption - Style.space(1)
          color: root.service.ready ? Commons.Color.accent : Commons.Color.urgent
          wrapMode: Text.WordWrap
        }

        RowLayout {
          Layout.fillWidth: true
          Layout.topMargin: Style.space(2)
          spacing: Style.space(6)

          EngineAction {
            text: "Reiniciar"
            iconText: "󰑐"
            tooltipText: "Restart rclone: drives mount again and syncs resume"
            enabled: root.service.ready && !root.service.busy
            onClicked: root.service.restartEngine()
          }

          EngineAction {
            text: "Logs"
            iconText: "󰈙"
            tooltipText: "What CloudKit and rclone did recently"
            enabled: root.service.ready
            onClicked: root.logRequested()
          }

          EngineAction {
            text: "Filtros"
            iconText: "󰈲"
            tooltipText: "Files that are never synced, by any folder without its own list. Changing them makes the next syncs safe resyncs."
            enabled: root.service.ready
            active: root.editingFilters
            onClicked: root.editingFilters ? (root.editingFilters = false) : root.openFilters()
          }

          EngineAction {
            text: "Backups"
            iconText: "󰁯"
            tooltipText: root.service.backupFiles > 0
                         ? root.service.backupFiles + " local files that syncs replaced or deleted, kept " + (root.s.backup_days || 30) + " days"
                         : "Local files that syncs replace or delete are kept here for " + (root.s.backup_days || 30) + " days"
            enabled: (root.service.statePaths.backups || "") !== ""
            onClicked: {
              if (root.service.backupFiles > 0) root.service.openPath(root.service.statePaths.backups)
              else root.service.notice("No local file has been replaced yet. When a sync overwrites or deletes one, the old copy is kept here for "
                                       + (root.s.backup_days || 30) + " days.")
            }
          }
        }

        // Filters: the global never-synced patterns, kept in filters.txt
        ColumnLayout {
          visible: root.editingFilters
          Layout.fillWidth: true
          Layout.topMargin: Style.space(6)
          spacing: Style.space(8)

          RowLayout {
            Layout.fillWidth: true
            spacing: Style.space(8)

            ColumnLayout {
              Layout.fillWidth: true
              spacing: 0
              Label { text: "Never synced, in all folders" }
              Hint {
                text: root.ownListFolders.length === 0
                      ? "Matching files and folders stay where they are: not uploaded, not downloaded."
                      : (root.ownListFolders.length === 1 ? root.ownListFolders[0] + " has"
                         : root.ownListFolders.slice(0, 2).join(" and ") + (root.ownListFolders.length > 2 ? " and " + (root.ownListFolders.length - 2) + " more" : "") + " have")
                        + " a list of " + (root.ownListFolders.length === 1 ? "its" : "their") + " own, which this doesn't change."
              }
            }

            Button {
              text: "filters.txt"
              iconText: "󰏫"
              tooltipText: "Open the file in your editor, for rules beyond never-sync patterns (rclone filter syntax)"
              fontFamily: root.fontFamily
              fontSize: Style.font.caption - Style.space(1)
              foreground: root.foreground
              bordered: false
              enabled: (root.service.statePaths.filters || "") !== ""
              onClicked: root.service.openPath(root.service.statePaths.filters)
            }
          }

          ExcludeEditor {
            id: filterEditor
            Layout.fillWidth: true
            foreground: root.foreground
            fontFamily: root.fontFamily
          }

          Hint {
            visible: (root.service.filters.otherRules || 0) > 0
            text: "filters.txt also has " + root.service.filters.otherRules
                  + (root.service.filters.otherRules === 1 ? " other rule" : " other rules") + ", kept as they are."
          }

          Text {
            visible: filterEditor.edited
            Layout.fillWidth: true
            textFormat: Text.PlainText
            text: Model.filterChangeText(filterEditor.added.length, filterEditor.removed.length, "", false)
            font.family: root.fontFamily
            font.pixelSize: Style.font.caption - Style.space(2)
            color: Commons.Color.accent
            wrapMode: Text.WordWrap
          }

          RowLayout {
            Layout.fillWidth: true
            spacing: Style.space(8)

            Button {
              text: "Save"
              iconText: "󰄬"
              fontFamily: root.fontFamily
              fontSize: Style.font.caption
              foreground: root.foreground
              bordered: true
              enabled: filterEditor.edited && !filterEditor.blocked
              opacity: enabled ? 1.0 : 0.45
              onClicked: {
                root.service.setExcludes("", filterEditor.result)
                root.editingFilters = false
              }
            }

            Item { Layout.fillWidth: true }

            Button {
              text: filterEditor.edited ? "Cancel" : "Close"
              fontFamily: root.fontFamily
              fontSize: Style.font.caption
              foreground: root.foreground
              bordered: false
              onClicked: root.editingFilters = false
            }
          }
        }
      }
    }
  }
}
