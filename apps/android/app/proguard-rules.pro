# The Rust core is called through JNA, which finds classes, fields and
# methods by name, from the generated bindings and from native code.
-keep class com.sun.jna.** { *; }
-keep class * implements com.sun.jna.** { *; }
-keepclassmembers class * extends com.sun.jna.** { public *; }
-keep class uniffi.cortex_mobile.** { *; }
-dontwarn java.awt.**
-dontwarn com.sun.jna.**

# The worker is made by WorkManager from its name.
-keep class dev.cortex.remote.RestWorker { <init>(...); }
