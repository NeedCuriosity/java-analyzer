//- /org/jspecify/annotations/Nullable.java lib:api
package org.jspecify.annotations;

import java.lang.annotation.*;

@Retention(RetentionPolicy.RUNTIME)
@Target(ElementType.TYPE_USE)
public @interface Nullable {}
//- /org/jspecify/annotations/NullMarked.java lib:api
package org.jspecify.annotations;

import java.lang.annotation.*;

@Retention(RetentionPolicy.RUNTIME)
@Target({ElementType.MODULE, ElementType.PACKAGE, ElementType.TYPE, ElementType.METHOD, ElementType.CONSTRUCTOR})
public @interface NullMarked {}
//- /org/jspecify/annotations/NullUnmarked.java lib:api
package org.jspecify.annotations;

import java.lang.annotation.*;

@Retention(RetentionPolicy.RUNTIME)
@Target({ElementType.PACKAGE, ElementType.TYPE, ElementType.METHOD, ElementType.CONSTRUCTOR})
public @interface NullUnmarked {}
//- /org/springframework/lang/NonNullApi.java lib:api
package org.springframework.lang;

import java.lang.annotation.*;

@Retention(RetentionPolicy.RUNTIME)
@Target(ElementType.PACKAGE)
public @interface NonNullApi {}
//- /org/springframework/lang/Nullable.java lib:api
package org.springframework.lang;

import java.lang.annotation.*;

@Retention(RetentionPolicy.RUNTIME)
@Target({ElementType.METHOD, ElementType.PARAMETER, ElementType.FIELD})
public @interface Nullable {}
//- /javax/annotation/ParametersAreNonnullByDefault.java lib:api
package javax.annotation;

import java.lang.annotation.*;

@Retention(RetentionPolicy.RUNTIME)
@Target({ElementType.PACKAGE, ElementType.TYPE})
public @interface ParametersAreNonnullByDefault {}
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
//- /org/checkerframework/checker/nullness/qual/NonNull.java lib:api
package org.checkerframework.checker.nullness.qual;

import java.lang.annotation.*;

@Retention(RetentionPolicy.RUNTIME)
@Target(ElementType.TYPE_USE)
public @interface NonNull {}
//- /org/checkerframework/framework/qual/TypeUseLocation.java lib:api
package org.checkerframework.framework.qual;

public enum TypeUseLocation {
    FIELD, LOCAL_VARIABLE, RESOURCE_VARIABLE, EXCEPTION_PARAMETER, RECEIVER, PARAMETER, RETURN, ALL
}
//- /org/checkerframework/framework/qual/DefaultQualifier.java lib:api
package org.checkerframework.framework.qual;

import java.lang.annotation.*;

@Retention(RetentionPolicy.CLASS)
@Target({ElementType.PACKAGE, ElementType.TYPE, ElementType.CONSTRUCTOR, ElementType.METHOD})
public @interface DefaultQualifier {
    Class<? extends Annotation> value();
    TypeUseLocation[] locations() default {TypeUseLocation.ALL};
}
//- /com/lib/marked/package-info.java lib:api
@NullMarked
package com.lib.marked;

import org.jspecify.annotations.NullMarked;
//- /com/lib/marked/Api.java lib:api
package com.lib.marked;

import java.util.List;
import org.jspecify.annotations.NullUnmarked;
import org.jspecify.annotations.Nullable;

public class Api {
    public Api(String name) {}

    public String name() { return "api"; }
    public @Nullable String find(String key) { return null; }
    public void take(String s) {}
    public void takeMaybe(@Nullable String s) {}
    public void pair(@Nullable String first, String second) {}
    public <T> void generic(T t) {}
    public void varargs(String... rest) {}
    public void elements(@Nullable String[] values) {}
    public void list(List<@Nullable String> values) {}
    @NullUnmarked
    public void legacy(String s) {}

    public static class Nested {
        public static void take(String s) {}
    }
}
//- /com/lib/marked/Legacy.java lib:api
package com.lib.marked;

import org.jspecify.annotations.NullUnmarked;

@NullUnmarked
public class Legacy {
    public static void take(String s) {}
}
//- /com/lib/spring/package-info.java lib:api
@NonNullApi
package com.lib.spring;

import org.springframework.lang.NonNullApi;
//- /com/lib/spring/Service.java lib:api
package com.lib.spring;

import org.springframework.lang.Nullable;

public interface Service {
    void save(String id, @Nullable Object value);
}
//- /com/lib/jsr305/Params.java lib:api
package com.lib.jsr305;

import javax.annotation.ParametersAreNonnullByDefault;

@ParametersAreNonnullByDefault
public class Params {
    public static void take(String s) {}
}
//- /com/lib/eclipse/Eclipse.java lib:api
package com.lib.eclipse;

import org.eclipse.jdt.annotation.DefaultLocation;
import org.eclipse.jdt.annotation.NonNullByDefault;
import org.eclipse.jdt.annotation.Nullable;

@NonNullByDefault
public class Eclipse {
    public static void take(String s) {}
    public static void takeMaybe(@Nullable String s) {}
    @NonNullByDefault({})
    public static void cancelled(String s) {}
    @NonNullByDefault(DefaultLocation.RETURN_TYPE)
    public static void returnsOnly(String s) {}
}
//- /com/lib/checker/Checker.java lib:api
package com.lib.checker;

import org.checkerframework.checker.nullness.qual.NonNull;
import org.checkerframework.framework.qual.DefaultQualifier;
import org.checkerframework.framework.qual.TypeUseLocation;

@DefaultQualifier(value = NonNull.class, locations = TypeUseLocation.PARAMETER)
public class Checker {
    public static void take(String s) {}
}
//- /com/lib/plain/Plain.java lib:api
package com.lib.plain;

public class Plain {
    public static void take(String s) {}
}
//- /app/Main.java
package app;

import com.lib.checker.Checker;
import com.lib.eclipse.Eclipse;
import com.lib.jsr305.Params;
import com.lib.marked.Api;
import com.lib.marked.Legacy;
import com.lib.plain.Plain;
import com.lib.spring.Service;

class Main {
    void calls(Api api, Service service) {
        api.take(/*@marked_arg*/null);
        api.take(/*@marked_maybe*/api.find("k"));
        api.list(/*@type_arg*/null);
        new Api(/*@marked_ctor*/null);
        Api.Nested.take(/*@nested_arg*/null);
        api.name().length();
        api.takeMaybe(null);
        api.pair(null, "x");
        api.pair("x", /*@pair_second*/null);
        api.generic(null);
        api.varargs(null, null);
        api.elements(null);
        api.legacy(null);
        Legacy.take(null);
        service.save(/*@spring_arg*/null, null);
        Params.take(/*@jsr305_arg*/null);
        Eclipse.take(/*@eclipse_arg*/null);
        Eclipse.takeMaybe(null);
        Eclipse.cancelled(null);
        Eclipse.returnsOnly(null);
        Checker.take(/*@checker_arg*/null);
        Plain.take(null);
    }
}
//? diag nullability-violation marked_arg
//? diag nullability-violation marked_maybe
//? diag nullability-violation type_arg
//? diag nullability-violation marked_ctor
//? diag nullability-violation pair_second
//? diag nullability-violation nested_arg
//? diag nullability-violation spring_arg
//? diag nullability-violation jsr305_arg
//? diag nullability-violation eclipse_arg
//? diag nullability-violation checker_arg
//? diag only nullability-violation
//? diag only nullable-dereference
//? diag only null-dereference
