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
//- /javax/annotation/ParametersAreNonnullByDefault.java lib:api
package javax.annotation;

import java.lang.annotation.*;

@Retention(RetentionPolicy.RUNTIME)
@Target({ElementType.PACKAGE, ElementType.TYPE})
public @interface ParametersAreNonnullByDefault {}
//- /com/example/ParametricNullness.java lib:api
package com.example;

import java.lang.annotation.*;

@Retention(RetentionPolicy.CLASS)
@Target({ElementType.METHOD, ElementType.PARAMETER, ElementType.FIELD})
public @interface ParametricNullness {}
//- /app/package-info.java
@NullMarked
package app;

import org.jspecify.annotations.NullMarked;
//- /app/Main.java
package app;

import com.example.ParametricNullness;
import java.util.List;
import java.util.function.Supplier;
import org.jspecify.annotations.NullUnmarked;
import org.jspecify.annotations.Nullable;

class Main<T> {
    @Nullable String cache;

    String plain() {
        return /*@default_return*/null;
    }

    @Nullable String maybe() { return null; }

    <U> U generic() { return null; }

    T classParam() { return null; }

    @ParametricNullness String parametric() { return null; }

    List<@Nullable String> list() {
        return /*@type_arg_return*/null;
    }

    String fromField() {
        return /*@default_return_maybe*/cache;
    }

    String lazy() {
        if (cache == null) {
            cache = "x";
        }
        return cache;
    }

    @NullUnmarked
    String legacyMethod() { return null; }

    // 没写注解的形参默认不为 null，但不能因此认为判空是多余的
    String safe(String s) {
        if (s == null) {
            return "";
        }
        return s;
    }

    void take(String s) {}
    void takeMaybe(@Nullable String s) {}
    <U> void takeGeneric(U u) {}
    void takeParam(T t) {}
    void takeParametric(@ParametricNullness String s) {}
    void takeVarargs(String... rest) {}

    void calls() {
        take(/*@default_arg*/null);
        take(/*@default_arg_maybe*/maybe());
        takeMaybe(null);
        takeGeneric(null);
        takeParam(null);
        takeParametric(null);
        takeVarargs(null, null);
        new Box(/*@default_ctor*/null);
        new Legacy().get(null);
        Supplier<String> supplier = () -> null;
        Runnable r = () -> {
            return;
        };
    }

    @NullUnmarked
    static class Legacy {
        String get(String key) { return null; }
    }

    static class Nested {
        String value() {
            return /*@nested_return*/null;
        }
    }
}

class Box {
    Box(String value) {}
}
//- /other/Params.java
package other;

import javax.annotation.ParametersAreNonnullByDefault;

@ParametersAreNonnullByDefault
class Params {
    void take(String s) {}

    // 只规定了形参，返回值不受影响
    String get() { return null; }

    void calls() {
        take(/*@jsr305_arg*/null);
    }
}

class Unmarked {
    String get() { return null; }
    void take(String s) {}
    void calls() { take(null); }
}
//? diag nullability-violation default_return
//? diag nullability-violation type_arg_return
//? diag nullability-violation default_return_maybe
//? diag nullability-violation default_arg
//? diag nullability-violation default_arg_maybe
//? diag nullability-violation default_ctor
//? diag nullability-violation nested_return
//? diag nullability-violation jsr305_arg
//? diag only nullability-violation
//? diag only nullable-dereference
//? diag only null-dereference
//? diag only constant-condition
