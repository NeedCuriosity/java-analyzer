//- /org/eclipse/jdt/annotation/DefaultLocation.java lib:api
package org.eclipse.jdt.annotation;

public enum DefaultLocation {
    PARAMETER, RETURN_TYPE, FIELD, TYPE_PARAMETER, TYPE_BOUND, TYPE_ARGUMENT, ARRAY_CONTENTS
}
//- /org/eclipse/jdt/annotation/NonNullByDefault.java lib:api
package org.eclipse.jdt.annotation;

import java.lang.annotation.*;
import static org.eclipse.jdt.annotation.DefaultLocation.*;

@Retention(RetentionPolicy.CLASS)
@Target({ElementType.PACKAGE, ElementType.TYPE, ElementType.METHOD, ElementType.CONSTRUCTOR, ElementType.FIELD, ElementType.LOCAL_VARIABLE})
public @interface NonNullByDefault {
    DefaultLocation[] value() default {PARAMETER, RETURN_TYPE, FIELD, TYPE_BOUND, TYPE_ARGUMENT};
}
//- /org/eclipse/jdt/annotation/Nullable.java lib:api
package org.eclipse.jdt.annotation;

import java.lang.annotation.*;

@Retention(RetentionPolicy.CLASS)
@Target(ElementType.TYPE_USE)
public @interface Nullable {}
//- /org/eclipse/jdt/annotation/NonNull.java lib:api
package org.eclipse.jdt.annotation;

import java.lang.annotation.*;

@Retention(RetentionPolicy.CLASS)
@Target(ElementType.TYPE_USE)
public @interface NonNull {}
//- /app/package-info.java
@NonNullByDefault
package app;

import org.eclipse.jdt.annotation.NonNullByDefault;
//- /app/Main.java
package app;

import org.eclipse.jdt.annotation.DefaultLocation;
import org.eclipse.jdt.annotation.NonNull;
import org.eclipse.jdt.annotation.NonNullByDefault;
import org.eclipse.jdt.annotation.Nullable;

class Main {
    String name() {
        return /*@default_return*/null;
    }

    @Nullable String find() { return null; }

    void take(String s) {}
    void takeMaybe(@Nullable String s) {}

    @NonNullByDefault({})
    String cancelled(String s) { return null; }

    @NonNullByDefault(DefaultLocation.PARAMETER)
    String paramsOnly(String s) { return null; }

    void calls() {
        take(/*@default_arg*/null);
        takeMaybe(null);
        cancelled(null);
        paramsOnly(/*@params_only_arg*/null);
        /*@deref*/find().length();
    }
}
//- /other/Explicit.java
package other;

import org.eclipse.jdt.annotation.NonNull;

class Explicit {
    void take(@NonNull String s) {}
    void plain(String s) {}

    void calls() {
        take(/*@explicit_arg*/null);
        plain(null);
    }
}
//? diag nullability-violation default_return
//? diag nullability-violation default_arg
//? diag nullability-violation params_only_arg
//? diag nullability-violation explicit_arg
//? diag nullable-dereference deref
//? diag only nullability-violation
//? diag only nullable-dereference
//? diag only null-dereference
