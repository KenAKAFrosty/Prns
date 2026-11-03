package rs.reticulum.prns.app.platform

import android.content.Context
import rs.reticulum.prns.chat.app.appInstallationDirectory
import java.nio.file.Files
import java.nio.file.LinkOption
import java.nio.file.NoSuchFileException
import java.nio.file.FileAlreadyExistsException
import java.nio.file.Path
import java.nio.file.attribute.BasicFileAttributes
import java.nio.file.attribute.PosixFilePermissions

internal object PrnsAppStorage {
  fun root(context: Context, create: Boolean): Path {
    val root = context.applicationContext.filesDir.toPath().resolve("prns-app")
    if (create) {
      try { Files.createDirectory(root, PosixFilePermissions.asFileAttribute(PosixFilePermissions.fromString("rwx------"))) }
      catch (_: FileAlreadyExistsException) { /* Rust validates existing root safety. */ }
    }
    return root
  }
  fun installationPresent(context: Context): Boolean = try {
    Files.readAttributes(root(context, false).resolve(appInstallationDirectory()), BasicFileAttributes::class.java, LinkOption.NOFOLLOW_LINKS)
    true
  } catch (_: NoSuchFileException) { false }
}
