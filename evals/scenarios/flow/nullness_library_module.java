//- /org/jspecify/annotations/NullMarked.java lib:modlib
package org.jspecify.annotations;

import java.lang.annotation.*;

@Retention(RetentionPolicy.RUNTIME)
@Target({ElementType.MODULE, ElementType.PACKAGE, ElementType.TYPE, ElementType.METHOD, ElementType.CONSTRUCTOR})
public @interface NullMarked {}
//- /org/jspecify/annotations/NullUnmarked.java lib:modlib
package org.jspecify.annotations;

import java.lang.annotation.*;

@Retention(RetentionPolicy.RUNTIME)
@Target({ElementType.PACKAGE, ElementType.TYPE, ElementType.METHOD, ElementType.CONSTRUCTOR})
public @interface NullUnmarked {}
//- /module-info.java lib:modlib
import org.jspecify.annotations.NullMarked;

@NullMarked
module com.lib.mod {
    exports com.lib.mod;
    exports com.lib.mod.legacy;
    exports org.jspecify.annotations;
}
//- /com/lib/mod/Store.java lib:modlib
package com.lib.mod;

public class Store {
    public static void put(String key) {}
}
//- /com/lib/mod/legacy/package-info.java lib:modlib
@NullUnmarked
package com.lib.mod.legacy;

import org.jspecify.annotations.NullUnmarked;
//- /com/lib/mod/legacy/Old.java lib:modlib
package com.lib.mod.legacy;

public class Old {
    public static void put(String key) {}
}
//- /app/Main.java
package app;

import com.lib.mod.Store;
import com.lib.mod.legacy.Old;

class Main {
    void calls() {
        Store.put(/*@module_arg*/null);
        Old.put(null);
    }
}
//? diag nullability-violation module_arg
//? diag only nullability-violation
