package dev.cortex.remote.ui

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import com.google.mlkit.vision.barcode.common.Barcode
import com.google.mlkit.vision.codescanner.GmsBarcodeScannerOptions
import com.google.mlkit.vision.codescanner.GmsBarcodeScanning
import dev.cortex.remote.AppModel

/** The first screen: what the app is, and how to pair it. */
@Composable
fun WelcomeScreen(model: AppModel) {
    var pairing by remember { mutableStateOf(false) }
    var appeared by remember { mutableStateOf(false) }

    val rise by animateFloatAsState(if (appeared) 1f else 0f, settle(), label = "appear")

    LaunchedEffect(Unit) { appeared = true }

    Column(
        Modifier
            .fillMaxSize()
            .statusBarsPadding()
            .navigationBarsPadding()
            .padding(horizontal = 24.dp, vertical = 20.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Spacer(Modifier.weight(1f))

        LogoBadge(
            size = 108.dp,
            alive = true,
            modifier = Modifier.graphicsLayer {
                scaleX = 0.8f + 0.2f * rise
                scaleY = 0.8f + 0.2f * rise
                alpha = rise
            },
        )

        Text("Cortex", style = Type.title(40), color = colors.text, modifier = Modifier.padding(top = 28.dp))

        Text(
            "Your coding agent, on this phone or on your computer.",
            style = Type.body,
            color = colors.textSecondary,
            textAlign = TextAlign.Center,
            modifier = Modifier.padding(top = 6.dp),
        )

        Column(
            Modifier
                .padding(top = 36.dp)
                .widthIn(max = 520.dp)
                .fillMaxWidth()
                .graphicsLayer {
                    alpha = rise
                    translationY = (1 - rise) * 40f
                }
                .card(18.dp),
            verticalArrangement = Arrangement.spacedBy(18.dp),
        ) {
            Step(1, "Open Cortex on your computer.")
            Step(2, "Go to Settings, then Remote control, and switch it on.")
            Step(3, "Show a pairing code and scan it here.")
        }

        Spacer(Modifier.weight(1f))

        ConnectionNote(model.state.connection, Modifier.padding(bottom = 12.dp))

        PrimaryButton("Work on this phone") { model.core.usePhone() }

        SecondaryButton("Pair with a computer", Modifier.padding(top = 10.dp)) { pairing = true }

        Text(
            "On this phone the agent, its memory and its networks run here. Paired, they stay on your computer and the connection is encrypted end to end.",
            style = Type.footnote,
            color = colors.textMuted,
            textAlign = TextAlign.Center,
            modifier = Modifier.padding(top = 14.dp),
        )
    }

    if (pairing) {
        PairingSheet(model) { pairing = false }
    }
}

@Composable
private fun Step(number: Int, text: String) {
    Row(horizontalArrangement = Arrangement.spacedBy(14.dp), verticalAlignment = Alignment.Top) {
        Box(
            Modifier
                .size(24.dp)
                .clip(CircleShape)
                .background(colors.accent.copy(alpha = 0.14f)),
            contentAlignment = Alignment.Center,
        ) {
            Text("$number", style = Type.footnote.copy(fontWeight = FontWeight.Bold), color = colors.accent)
        }

        Text(text, style = Type.callout, color = colors.text, modifier = Modifier.padding(top = 2.dp))
    }
}

/** Scanning the code on the computer's screen, or pasting it as text. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun PairingSheet(model: AppModel, close: () -> Unit) {
    val context = LocalContext.current
    val view = LocalView.current

    var typed by remember { mutableStateOf("") }
    var problem by remember { mutableStateOf<String?>(null) }

    // Hosts known when the sheet opened: one more means it worked.
    val known = remember { model.state.hosts.size }

    LaunchedEffect(model.state.hosts.size, model.connected) {
        if (model.state.hosts.size > known || model.connected) {
            Haptics.confirm(view)
            close()
        }
    }

    fun pair(code: String) {
        val trimmed = code.trim()

        if (trimmed.startsWith("cortex-pair:")) {
            problem = null
            model.core.pair(trimmed)
        } else {
            problem = "That is not a Cortex pairing code."
        }
    }

    ModalBottomSheet(
        onDismissRequest = close,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = colors.background,
    ) {
        Column(
            Modifier
                .padding(horizontal = 24.dp)
                .padding(bottom = 28.dp)
                .navigationBarsPadding(),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Text("Pair with a computer", style = Type.title(24), color = colors.text)

            Text(
                "Point the camera at the code on your computer's screen.",
                style = Type.callout,
                color = colors.textSecondary,
            )

            PrimaryButton("Scan the code") {
                val options = GmsBarcodeScannerOptions.Builder()
                    .setBarcodeFormats(Barcode.FORMAT_QR_CODE)
                    .build()

                GmsBarcodeScanning.getClient(context, options)
                    .startScan()
                    .addOnSuccessListener { barcode -> barcode.rawValue?.let(::pair) }
                    .addOnFailureListener {
                        problem = "The camera could not be used. Enter the code as text instead."
                    }
            }

            Text(
                "Or choose “Copy as text” under the code on the computer, send it to this phone and paste it here.",
                style = Type.footnote,
                color = colors.textMuted,
            )

            Box(
                Modifier
                    .fillMaxWidth()
                    .clip(RoundedCornerShape(SmallRadius))
                    .background(colors.sunken)
                    .padding(12.dp),
            ) {
                if (typed.isEmpty()) {
                    Text("cortex-pair:…", style = Type.mono, color = colors.textMuted)
                }

                BasicTextField(
                    value = typed,
                    onValueChange = { typed = it },
                    textStyle = Type.mono.copy(color = colors.text),
                    cursorBrush = SolidColor(colors.accent),
                    maxLines = 6,
                    modifier = Modifier.fillMaxWidth(),
                )
            }

            SecondaryButton("Pair with this code", enabled = typed.isNotBlank()) { pair(typed) }

            problem?.let { Text(it, style = Type.footnote, color = colors.bad) }

            ConnectionNote(model.state.connection, Modifier.fillMaxWidth())
        }
    }
}
