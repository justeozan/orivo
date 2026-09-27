package io.orivo.saf

import android.app.Activity
import android.content.Intent
import android.net.Uri
import androidx.activity.result.ActivityResult
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

/**
 * One activity result, handed to Rust as a string.
 *
 * Orivo's host does the reading, the bounding and the hashing. This class only
 * exists because `onActivityResult` has to land on a Java class, and because the
 * persistable permission can only be taken from the process that received the
 * grant — a later JNI call would find nothing to take.
 */
@TauriPlugin
class SafPlugin(private val activity: Activity) : Plugin(activity) {
    @Command
    fun pickDocumentTree(invoke: Invoke) {
        try {
            val intent = Intent(Intent.ACTION_OPEN_DOCUMENT_TREE)
            // Asking for the persistable flag here is what makes
            // takePersistableUriPermission() below legal at all.
            intent.addFlags(
                Intent.FLAG_GRANT_READ_URI_PERMISSION or
                    Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION
            )
            startActivityForResult(invoke, intent, "onDocumentTreePicked")
        } catch (ex: Exception) {
            invoke.reject(ex.message)
        }
    }

    @ActivityCallback
    fun onDocumentTreePicked(invoke: Invoke, result: ActivityResult) {
        val answer = JSObject()
        val tree: Uri? =
            if (result.resultCode == Activity.RESULT_OK) result.data?.data else null
        if (tree == null) {
            // Backing out of the chooser is an ordinary outcome, not an error.
            // The key is left out rather than set to null, because JSONObject
            // drops a null value anyway and the host reads an absent tree as
            // "nothing was connected".
            invoke.resolve(answer)
            return
        }
        try {
            // Read only. Orivo never writes into the folder Winlator exports to.
            activity.contentResolver.takePersistableUriPermission(
                tree,
                Intent.FLAG_GRANT_READ_URI_PERMISSION
            )
        } catch (ex: SecurityException) {
            // The grant exists for this task either way, but a grant Orivo
            // cannot keep is worse than none: the user would connect a folder
            // once and silently lose it at the next start.
            invoke.reject("Android did not let Orivo keep this folder: ${ex.message}")
            return
        }
        answer.put("treeUri", tree.toString())
        invoke.resolve(answer)
    }
}
