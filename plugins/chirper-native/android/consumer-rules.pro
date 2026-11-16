# The plugin classes are looked up by name from the Rust side (register_android_plugin) and the
# Tauri bridge reads @InvokeArg classes with Jackson. Keep both.
-keep class app.chirper.platform.** { *; }
