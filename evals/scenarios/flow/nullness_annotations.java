//- /org/jetbrains/annotations/Nullable.java lib:api
package org.jetbrains.annotations;

import java.lang.annotation.*;

@Retention(RetentionPolicy.CLASS)
@Target({ElementType.METHOD, ElementType.FIELD, ElementType.PARAMETER, ElementType.LOCAL_VARIABLE, ElementType.TYPE_USE})
public @interface Nullable {}
//- /org/jetbrains/annotations/NotNull.java lib:api
package org.jetbrains.annotations;

import java.lang.annotation.*;

@Retention(RetentionPolicy.CLASS)
@Target({ElementType.METHOD, ElementType.FIELD, ElementType.PARAMETER, ElementType.LOCAL_VARIABLE, ElementType.TYPE_USE})
public @interface NotNull {}
//- /org/jspecify/annotations/Nullable.java lib:api
package org.jspecify.annotations;

import java.lang.annotation.*;

@Retention(RetentionPolicy.RUNTIME)
@Target(ElementType.TYPE_USE)
public @interface Nullable {}
//- /org/lib/Api.java lib:api
package org.lib;

import org.jetbrains.annotations.NotNull;
import org.jetbrains.annotations.Nullable;

public class Api {
    public @Nullable String find(String key) { return null; }
    public String plain(String key) { return key; }
    public @org.jspecify.annotations.Nullable String typeUse() { return null; }
    public @Nullable String cache;
    public void accept(@NotNull String value, String other) {}
    public Api(@NotNull String name) {}
    public class Inner {
        public Inner(String a, @NotNull String b) {}
    }
}
//- /app/Main.java
package app;

import org.jetbrains.annotations.NotNull;
import org.jetbrains.annotations.Nullable;
import org.lib.Api;

class Main {
    @Nullable String parent() { return null; }
    @Nullable String nullableField;

    int library(Api api) {
        String s = api.find("k");
        int n = /*@lib_local*/s.length();
        n += api./*@lib_direct*/find("a").length();
        n += api./*@type_use*/typeUse().length();
        n += api.plain("k").length();
        String c = api.cache;
        return n + /*@lib_field*/c.length();
    }

    int source(@Nullable String param, boolean flag) {
        int n = /*@param*/param.length();
        n += /*@own_direct*/parent().length();
        String f = nullableField;
        n += /*@own_field*/f.length();
        String joined = flag ? "x" : parent();
        return n + /*@joined*/joined.length();
    }

    int checked(@Nullable String a, Api api) {
        if (a == null) {
            return 0;
        }
        int n = a.length();
        String b = api.find("b");
        if (b != null) {
            n += b.length();
        }
        String c = api.find("c");
        if (c == null) c = "";
        n += c.length();
        if (parent() != null) {
            n += parent().length();
        }
        String d = api.find("d");
        if (isEmpty(d)) return n;
        n += d.length();
        String e = api.find("e");
        java.util.Objects.requireNonNull(e);
        n += e.length();
        String g = api.find("g");
        if (g == null || g.isEmpty()) return n;
        // 检查过的值不报"条件恒定"
        String h = api.find("h");
        if (h != null) n++;
        return n;
    }

    static boolean isEmpty(String s) { return s == null || s.isEmpty(); }

    @Nullable Object node;
    @Nullable Main next;

    int fieldsChecked() {
        if (node instanceof String) {
            String str = (String) node;
            return str.length();
        }
        check(next != null);
        Main n = next;
        int h = n.hashCode();
        Main cur;
        while ((cur = next) != null) {
            h += cur.hashCode();
        }
        return h;
    }

    static void check(boolean b) {}

    @Nullable Object[] elements;
    Object @Nullable [] maybeArray;

    int arrays(@Nullable Object... rest) {
        // 修饰符位置的注解修饰数组元素，数组本身不为 null
        Object[] e = elements;
        int n = e.length + rest.length;
        Object[] m = maybeArray;
        return n + /*@array_itself*/m.length;
    }

    void notNullParams(Api api, @Nullable String maybe) {
        api.accept(/*@pass_null*/null, null);
        api.accept(/*@pass_maybe*/maybe, null);
        api.accept("x", null);
        new Api(/*@ctor_null*/null);
        api.new Inner(null, /*@inner_null*/null);
        take(/*@own_param*/null);
    }

    void take(@NotNull String s) {}

    @NotNull String returnsNull(boolean flag) {
        if (flag) {
            return /*@return_null*/null;
        }
        Runnable r = () -> {
            return;
        };
        java.util.function.Supplier<String> sup = () -> null;
        return /*@return_maybe*/parent();
    }
}
//? diag nullable-dereference lib_local
//? diag nullable-dereference lib_direct
//? diag nullable-dereference type_use
//? diag nullable-dereference lib_field
//? diag nullable-dereference param
//? diag nullable-dereference own_direct
//? diag nullable-dereference own_field
//? diag nullable-dereference joined
//? diag nullable-dereference array_itself
//? diag only nullable-dereference
//? diag nullability-violation pass_null
//? diag nullability-violation pass_maybe
//? diag nullability-violation ctor_null
//? diag nullability-violation inner_null
//? diag nullability-violation own_param
//? diag nullability-violation return_null
//? diag nullability-violation return_maybe
//? diag only nullability-violation
//? diag only constant-condition
