package dev.cortex.remote.ui

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.KeyboardArrowRight
import androidx.compose.material.icons.rounded.Add
import androidx.compose.material.icons.rounded.ChatBubble
import androidx.compose.material.icons.rounded.Computer
import androidx.compose.material.icons.rounded.Settings
import androidx.compose.material.icons.rounded.Folder
import androidx.compose.material.icons.rounded.Forum
import androidx.compose.material.icons.rounded.PanTool
import androidx.compose.material.icons.rounded.PriorityHigh
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dev.cortex.remote.AppModel
import dev.cortex.remote.ui.settings.SettingsScreen
import uniffi.cortex_mobile.ChatView
import uniffi.cortex_mobile.Connection
import uniffi.cortex_mobile.ProjectView
import java.text.DateFormat
import java.util.Date

/** Which screen is in front: the lists, a chat, or the settings over either. */
private fun depth(chat: String?, settings: String?) = when {
    settings != null -> 2
    chat != null -> 1
    else -> 0
}

/** The chats open on the computer, its projects, the chat in front, and the settings. */
@Composable
fun HomeScreen(model: AppModel) {
    var open by rememberSaveable { mutableStateOf<String?>(null) }

    // The card to unfold, "" for none; null while the settings are closed.
    var settings by rememberSaveable { mutableStateOf<String?>(null) }

    // A chat that is closed on the computer cannot stay open here.
    LaunchedEffect(model.state.chats, model.connected) {
        if (!model.connected || model.state.chats.none { it.id == open }) {
            open = null
        }
    }

    LaunchedEffect(model.stage, model.state.chats.size) {
        val first = model.state.chats.firstOrNull()?.id

        when (model.stage) {
            null -> {}
            "phone" -> settings = "account"
            "settings" -> settings = ""
            "themes" -> settings = "appearance"
            "desktops" -> settings = "desktops"

            "capabilities" -> {
                open = first
                settings = "capabilities"
            }

            else -> open = first
        }
    }

    AnimatedContent(
        targetState = Triple(open, settings, depth(open, settings)),
        transitionSpec = {
            if (targetState.third >= initialState.third) {
                (slideInHorizontally(settle()) { it / 3 } + fadeIn()) togetherWith
                    (slideOutHorizontally(settle()) { -it / 6 } + fadeOut())
            } else {
                (slideInHorizontally(settle()) { -it / 6 } + fadeIn()) togetherWith
                    (slideOutHorizontally(settle()) { it / 3 } + fadeOut())
            }
        },
        contentKey = { it.third },
        label = "screen",
    ) { (chat, focus, _) ->
        when {
            focus != null -> BackSwipe(back = { settings = null; model.stage = null }) { swipe ->
                Box(swipe) {
                    SettingsScreen(model, chat, focus.ifEmpty { null }) {
                        settings = null
                        model.stage = null
                    }
                }
            }

            chat != null -> BackSwipe(back = { open = null }) { swipe ->
                Box(swipe) {
                    ConversationScreen(model, chat, back = { open = null }) { card -> settings = card ?: "" }
                }
            }

            else -> Lists(model, open = { open = it }) { card -> settings = card ?: "" }
        }
    }
}

@Composable
private fun Lists(model: AppModel, open: (String) -> Unit, openSettings: (String?) -> Unit) {
    var naming by remember { mutableStateOf(false) }

    Column(
        Modifier
            .fillMaxSize()
            .background(colors.background)
            .statusBarsPadding()
            .verticalScroll(rememberScrollState())
            .measure()
            .padding(horizontal = Gutter)
            .padding(bottom = Space.xxl)
            .navigationBarsPadding(),
        verticalArrangement = Arrangement.spacedBy(Space.xl),
    ) {
        Row(
            Modifier.padding(top = Space.m),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            LogoBadge(size = 44.dp)

            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                Text("Cortex", style = Type.title(26), color = colors.text)

                ConnectionPill(model.state.connection)
            }

            RoundButton(Icons.Rounded.Computer, "Where the agent runs") { openSettings("desktops") }
            RoundButton(Icons.Rounded.Settings, "Settings") { openSettings(null) }
        }

        if (model.connected) {
            Column(verticalArrangement = Arrangement.spacedBy(Space.m)) {
                SectionHeading(if (model.state.onPhone) "On this phone" else "Open on your computer")

                if (model.state.chats.isEmpty()) {
                    Text("No chats yet.", style = Type.callout, color = colors.textMuted)
                }

                for (chat in model.state.chats) {
                    ChatCard(chat) { open(chat.id) }
                }
            }

            Column(verticalArrangement = Arrangement.spacedBy(Space.m)) {
                Line {
                    SectionHeading("Projects and past sessions", Modifier.weight(1f))

                    if (model.state.onPhone) {
                        Line(Modifier.pressable { naming = true }, gap = Space.xs) {
                            Icon(Icons.Rounded.Add, null, tint = colors.accent, modifier = Modifier.size(16.dp))
                            Text("New project", style = Type.footnote, color = colors.accent)
                        }
                    }
                }

                if (model.state.projects.isEmpty()) {
                    Text("None yet.", style = Type.callout, color = colors.textMuted)
                }

                for (project in model.state.projects) {
                    ProjectCard(model, project, open)
                }
            }
        } else {
            NotConnected(model) { openSettings("desktops") }
        }
    }

    if (naming) {
        NewProjectSheet(model) { naming = false }
    }
}

@Composable
private fun RoundButton(icon: androidx.compose.ui.graphics.vector.ImageVector, label: String, onClick: () -> Unit) {
    Box(
        Modifier
            .size(44.dp)
            .pressable(onClick = onClick)
            .clip(CircleShape)
            .background(colors.surface)
            .border(1.dp, colors.border.copy(alpha = 0.7f), CircleShape),
        contentAlignment = Alignment.Center,
    ) {
        Icon(icon, label, tint = colors.text, modifier = Modifier.size(20.dp))
    }
}

/** A name for a new project on this phone, which is then worked in. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun NewProjectSheet(model: AppModel, close: () -> Unit) {
    var name by remember { mutableStateOf("") }

    ModalBottomSheet(onDismissRequest = close, containerColor = colors.background) {
        Column(
            Modifier
                .padding(horizontal = Gutter)
                .padding(bottom = Space.xl)
                .navigationBarsPadding()
                .imePadding(),
            verticalArrangement = Arrangement.spacedBy(Space.m),
        ) {
            Text("New project", style = Type.title(22), color = colors.text)

            Text("A folder on this phone for the agent to work in.", style = Type.callout, color = colors.textSecondary)

            dev.cortex.remote.ui.settings.Field(name, "Name") { name = it }

            PrimaryButton("Create and open", enabled = name.isNotBlank()) {
                model.core.newProject(name.trim())
                close()
            }
        }
    }
}

@Composable
private fun ChatCard(chat: ChatView, open: () -> Unit) {
    val tint = when {
        chat.asking -> colors.warning
        chat.problem != null -> colors.bad
        chat.running -> colors.accent
        else -> colors.model
    }

    val status = when {
        chat.asking -> "Needs your answer"
        chat.running -> "Working"
        chat.problem != null -> "Not set up"
        !chat.ready -> "Starting"
        chat.model.isEmpty() -> "Ready"
        else -> chat.model
    }

    val icon = when {
        chat.asking -> Icons.Rounded.PanTool
        chat.problem != null -> Icons.Rounded.PriorityHigh
        else -> Icons.Rounded.ChatBubble
    }

    Row(
        Modifier
            .fillMaxWidth()
            .pressable(onClick = open)
            .card(),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Box(Modifier.size(42.dp), contentAlignment = Alignment.Center) {
            Box(Modifier.fillMaxSize().clip(CircleShape).background(tint.copy(alpha = 0.15f)))

            if (chat.running && !chat.asking) {
                RunningRing(tint, Modifier.fillMaxSize())
            }

            Icon(icon, null, tint = tint, modifier = Modifier.size(18.dp))
        }

        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(3.dp)) {
            Text(chat.title, style = Type.bodyStrong, color = colors.text, maxLines = 1, overflow = TextOverflow.Ellipsis)

            Line(gap = 6.dp) {
                Text(status, style = Type.footnote, color = if (chat.asking) colors.warning else colors.textSecondary, maxLines = 1)
                Text("·", style = Type.footnote, color = colors.textMuted)
                Text(chat.placeName, style = Type.footnote, color = colors.textMuted, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }

        Icon(Icons.AutoMirrored.Rounded.KeyboardArrowRight, null, tint = colors.textMuted)
    }
}

@Composable
private fun ProjectCard(model: AppModel, project: ProjectView, open: (String) -> Unit) {
    var expanded by rememberSaveable(project.path) { mutableStateOf(false) }

    val turn by animateFloatAsState(if (expanded) 90f else 0f, settle(), label = "chevron")

    // The chat that sessions are opened in: the first that is free.
    val target = model.state.chats.firstOrNull { !it.running } ?: model.state.chats.firstOrNull()
    val free = target != null && !target.running

    Column(Modifier.fillMaxWidth().card(12.dp)) {
        Row(
            Modifier
                .fillMaxWidth()
                .heightIn(min = 44.dp)
                .pressable { expanded = !expanded },
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Icon(
                if (project.scratch) Icons.Rounded.Forum else Icons.Rounded.Folder,
                null,
                tint = colors.textSecondary,
                modifier = Modifier.size(20.dp),
            )

            Text(
                if (project.scratch) "Chats" else project.name,
                style = Type.bodyStrong,
                color = colors.text,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )

            Text("${project.sessions.size}", style = Type.footnote, color = colors.textMuted)

            Icon(
                Icons.AutoMirrored.Rounded.KeyboardArrowRight,
                null,
                tint = colors.textMuted,
                modifier = Modifier.rotate(turn),
            )
        }

        AnimatedVisibility(expanded, enter = expandVertically(settle()) + fadeIn(), exit = shrinkVertically(settle()) + fadeOut()) {
            Column {
                HorizontalDivider(Modifier.padding(vertical = 6.dp), color = colors.border)

                Line(
                    Modifier
                        .fillMaxWidth()
                        .heightIn(min = 44.dp)
                        .pressable(free) {
                            model.core.newSession(target!!.id, project.path)
                            open(target.id)
                        },
                ) {
                    Icon(Icons.Rounded.Add, null, tint = colors.accent, modifier = Modifier.size(18.dp))
                    Text("New session here", style = Type.callout, color = colors.accent)
                }

                for (session in project.sessions) {
                    Row(
                        Modifier
                            .fillMaxWidth()
                            .heightIn(min = 44.dp)
                            .pressable(free) {
                                model.core.openSession(target!!.id, session.path)
                                open(target.id)
                            },
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text(
                            session.title.ifEmpty { "Untitled" },
                            style = Type.callout,
                            color = colors.text,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            modifier = Modifier.weight(1f),
                        )

                        Spacer(Modifier.width(8.dp))

                        Text(
                            DateFormat.getDateInstance(DateFormat.SHORT).format(Date(session.updatedMs.toLong())),
                            style = Type.caption,
                            color = colors.textMuted,
                        )
                    }
                }

                if (target?.running == true) {
                    Text(
                        "The chat is working. Sessions can be opened when it is done.",
                        style = Type.caption,
                        color = colors.textMuted,
                        modifier = Modifier.padding(top = 4.dp),
                    )
                }
            }
        }
    }
}

/** Shown instead of the lists while there is no connection. */
@Composable
private fun NotConnected(model: AppModel, showHosts: () -> Unit) {
    val connection = model.state.connection

    Column(
        Modifier.fillMaxWidth().padding(top = 40.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(18.dp),
    ) {
        LogoBadge(size = 72.dp, alive = true)

        Text(
            when (connection) {
                is Connection.Failed -> "Could not connect"
                is Connection.Idle -> "Not connected"
                else -> "Finding your computer"
            },
            style = Type.title(22),
            color = colors.text,
        )

        ConnectionNote(connection)

        Text(
            "Cortex has to be open on the computer, with remote control switched on in its settings.",
            style = Type.callout,
            color = colors.textSecondary,
            textAlign = TextAlign.Center,
        )

        val host = model.state.hosts.firstOrNull()

        if (connection is Connection.Idle && host != null) {
            PrimaryButton("Connect to ${host.name}") { model.core.connect(host.id) }
        }

        SecondaryButton("Work on this phone") { model.core.usePhone() }

        SecondaryButton("Computers and pairing", onClick = showHosts)
    }
}
