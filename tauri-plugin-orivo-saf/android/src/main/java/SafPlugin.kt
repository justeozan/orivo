package io.orivo.saf

import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.ComponentName
import android.content.Intent
import android.content.pm.ApplicationInfo
import android.content.pm.PackageInfo
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.LinearGradient
import android.graphics.Paint
import android.graphics.Path
import android.graphics.RectF
import android.graphics.Shader
import android.graphics.drawable.Drawable
import android.net.Uri
import android.os.Build
import androidx.activity.result.ActivityResult
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.File
import java.io.FileOutputStream

@InvokeArg
class LaunchPackageArgs {
    var packageName: String = ""
}

@InvokeArg
class StartStreamArgs {
    var pcName: String = ""
    var appName: String? = null
}

/**
 * The few things Orivo asks of Android that need a Java class to land on.
 *
 * Orivo's host does the deciding, the bounding and the hashing. This class only
 * exists for what JNI makes awkward or impossible:
 *
 * - `onActivityResult`, which the folder chooser answers through, and the
 *   persistable permission only the process that received the grant may take;
 * - the games installed on this phone, whose names and icons only
 *   `PackageManager` can give, and whose pictures have to be drawn here;
 * - starting one of them, through the launch intent its own package declares.
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

    /**
     * The games installed on this phone: every app with a launcher entry that
     * Android files under its game category. Each comes with its name and three
     * pictures drawn from its icon — the icon itself, a portrait cover and a
     * landscape wallpaper — written into Orivo's cache, where the WebView is
     * allowed to read them. Only file names leave this method.
     *
     * A picture is redrawn when the app was updated since it was drawn, so a
     * new icon reaches the library without anything else asking for it.
     */
    @Command
    fun listInstalledGames(invoke: Invoke) {
        try {
            val manager = activity.packageManager
            val launcher = Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_LAUNCHER)
            val entries =
                if (Build.VERSION.SDK_INT >= 33) {
                    manager.queryIntentActivities(launcher, PackageManager.ResolveInfoFlags.of(0))
                } else {
                    @Suppress("DEPRECATION")
                    manager.queryIntentActivities(launcher, 0)
                }
            val media = File(activity.cacheDir, "media")
            media.mkdirs()
            val games = JSArray()
            val seen = HashSet<String>()
            for (entry in entries) {
                val app = entry.activityInfo?.applicationInfo ?: continue
                val name = app.packageName ?: continue
                if (name == activity.packageName || !seen.add(name)) continue
                if (!isGame(app)) continue
                val label = app.loadLabel(manager)?.toString()?.trim().orEmpty()
                if (label.isEmpty()) continue
                val info = packageInfo(manager, name)
                val updatedAt = info?.lastUpdateTime ?: 0L
                val stem = "android-" + name.replace(Regex("[^A-Za-z0-9._-]"), "_")
                val icon = File(media, "$stem-icon.png")
                val cover = File(media, "$stem-cover.jpg")
                val hero = File(media, "$stem-hero.jpg")
                val stale = !icon.exists() || !cover.exists() || !hero.exists() ||
                    icon.lastModified() < updatedAt
                if (stale) {
                    try {
                        drawArt(app.loadIcon(manager), icon, cover, hero)
                    } catch (ex: Exception) {
                        // A game whose icon cannot be drawn is still a game; the
                        // library shows its name until a picture turns up.
                    }
                }
                val game = JSObject()
                game.put("packageName", name)
                game.put("label", label)
                if (icon.exists()) game.put("icon", icon.name)
                if (cover.exists()) game.put("cover", cover.name)
                if (hero.exists()) game.put("hero", hero.name)
                game.put("installedAt", info?.firstInstallTime ?: 0L)
                game.put("updatedAt", updatedAt)
                games.put(game)
            }
            val answer = JSObject()
            answer.put("games", games)
            invoke.resolve(answer)
        } catch (ex: Exception) {
            invoke.reject(ex.message)
        }
    }

    /** Start an installed app the way its own launcher icon does. */
    @Command
    fun launchPackage(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(LaunchPackageArgs::class.java)
            val intent = activity.packageManager.getLaunchIntentForPackage(args.packageName)
            if (intent == null) {
                invoke.reject("not-installed")
                return
            }
            intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            activity.startActivity(intent)
            invoke.resolve()
        } catch (ex: Exception) {
            invoke.reject(ex.message)
        }
    }

    /**
     * Stream a game from the player's PC through Moonlight. Moonlight's
     * shortcut entry point takes the PC by the name Moonlight itself shows and
     * the game by the name the PC's streaming host lists it under; with no game
     * it opens that PC's list, which is the honest fallback for a title the
     * host does not carry.
     */
    @Command
    fun startStream(invoke: Invoke) {
        try {
            val args = invoke.parseArgs(StartStreamArgs::class.java)
            // Moonlight, then the forks that kept its entry point under their
            // own package: Artemis (`.noir`) and the rooted build.
            for (pkg in listOf("com.limelight", "com.limelight.noir", "com.limelight.root")) {
                val intent = Intent()
                intent.component = ComponentName(pkg, "com.limelight.ShortcutTrampoline")
                intent.putExtra("Name", args.pcName)
                val appName = args.appName
                if (!appName.isNullOrBlank()) intent.putExtra("AppName", appName)
                intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                try {
                    activity.startActivity(intent)
                    invoke.resolve()
                    return
                } catch (ex: ActivityNotFoundException) {
                    continue
                }
            }
            invoke.reject("moonlight-missing")
        } catch (ex: Exception) {
            invoke.reject(ex.message)
        }
    }

    private fun isGame(app: ApplicationInfo): Boolean {
        if (Build.VERSION.SDK_INT >= 26 && app.category == ApplicationInfo.CATEGORY_GAME) {
            return true
        }
        @Suppress("DEPRECATION")
        return (app.flags and ApplicationInfo.FLAG_IS_GAME) != 0
    }

    private fun packageInfo(manager: PackageManager, name: String): PackageInfo? =
        try {
            if (Build.VERSION.SDK_INT >= 33) {
                manager.getPackageInfo(name, PackageManager.PackageInfoFlags.of(0))
            } else {
                @Suppress("DEPRECATION")
                manager.getPackageInfo(name, 0)
            }
        } catch (ex: PackageManager.NameNotFoundException) {
            null
        }

    /**
     * Three pictures from one icon. The cover and the wallpaper stand the icon
     * on a ground made of its own colours — the icon shrunk to a few pixels and
     * stretched back, which is a blur no library is needed for — darkened so
     * the library's text and glass read over it.
     */
    private fun drawArt(drawable: Drawable, icon: File, cover: File, hero: File) {
        val iconBitmap = render(drawable, 256)
        save(iconBitmap, icon, Bitmap.CompressFormat.PNG)

        val portrait = Bitmap.createBitmap(600, 900, Bitmap.Config.ARGB_8888)
        paintGround(Canvas(portrait), iconBitmap, 600, 900)
        paintIcon(Canvas(portrait), iconBitmap, 300f, 380f, 300f)
        save(portrait, cover, Bitmap.CompressFormat.JPEG)

        val landscape = Bitmap.createBitmap(1280, 720, Bitmap.Config.ARGB_8888)
        paintGround(Canvas(landscape), iconBitmap, 1280, 720)
        // Off-centre to the right: the Library writes the title on the left.
        paintIcon(Canvas(landscape), iconBitmap, 900f, 330f, 300f)
        save(landscape, hero, Bitmap.CompressFormat.JPEG)
    }

    private fun render(drawable: Drawable, size: Int): Bitmap {
        val bitmap = Bitmap.createBitmap(size, size, Bitmap.Config.ARGB_8888)
        val canvas = Canvas(bitmap)
        drawable.setBounds(0, 0, size, size)
        drawable.draw(canvas)
        return bitmap
    }

    private fun paintGround(canvas: Canvas, icon: Bitmap, width: Int, height: Int) {
        canvas.drawColor(Color.rgb(18, 17, 22))
        val tiny = Bitmap.createScaledBitmap(icon, 6, 6, true)
        val ground = Bitmap.createScaledBitmap(tiny, width, height, true)
        val paint = Paint(Paint.FILTER_BITMAP_FLAG)
        paint.alpha = 210
        canvas.drawBitmap(ground, 0f, 0f, paint)
        val shade = Paint()
        shade.shader = LinearGradient(
            0f, 0f, 0f, height.toFloat(),
            Color.argb(90, 8, 8, 12), Color.argb(200, 8, 8, 12),
            Shader.TileMode.CLAMP
        )
        canvas.drawRect(0f, 0f, width.toFloat(), height.toFloat(), shade)
    }

    private fun paintIcon(canvas: Canvas, icon: Bitmap, centreX: Float, centreY: Float, size: Float) {
        val bounds = RectF(centreX - size / 2, centreY - size / 2, centreX + size / 2, centreY + size / 2)
        val radius = size * 0.22f
        val shadow = Paint(Paint.ANTI_ALIAS_FLAG)
        shadow.color = Color.argb(150, 0, 0, 0)
        shadow.setShadowLayer(size * 0.12f, 0f, size * 0.05f, Color.argb(170, 0, 0, 0))
        canvas.drawRoundRect(bounds, radius, radius, shadow)
        val clip = Path()
        clip.addRoundRect(bounds, radius, radius, Path.Direction.CW)
        canvas.save()
        canvas.clipPath(clip)
        canvas.drawBitmap(icon, null, bounds, Paint(Paint.FILTER_BITMAP_FLAG or Paint.ANTI_ALIAS_FLAG))
        canvas.restore()
    }

    private fun save(bitmap: Bitmap, file: File, format: Bitmap.CompressFormat) {
        val partial = File(file.parentFile, file.name + ".part")
        FileOutputStream(partial).use { output ->
            bitmap.compress(format, if (format == Bitmap.CompressFormat.PNG) 100 else 88, output)
        }
        if (!partial.renameTo(file)) {
            file.delete()
            partial.renameTo(file)
        }
    }
}
