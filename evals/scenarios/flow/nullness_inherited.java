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
//- /org/lib/Base.java lib:api
package org.lib;

import org.jetbrains.annotations.NotNull;
import org.jetbrains.annotations.Nullable;

public class Base {
    public @Nullable String find(String key) { return null; }
    public void accept(@NotNull String value) {}
    public @NotNull String name() { return "base"; }
    public void put(@NotNull String value) {}
}
//- /org/lib/Fn.java lib:api
package org.lib;

import org.jetbrains.annotations.Nullable;

public interface Fn<F, T> {
    @Nullable T apply(@Nullable F input);
}
//- /app/Repo.java
package app;

import org.jetbrains.annotations.NotNull;
import org.jetbrains.annotations.Nullable;

interface Repo {
    @Nullable Object load(String key);
    void save(@NotNull Object value);
}
//- /app/Main.java
package app;

import org.jetbrains.annotations.NotNull;
import org.jetbrains.annotations.Nullable;
import org.lib.Base;
import org.lib.Fn;

class Sub extends Base {
    @Override
    public String find(String key) { return key; }
    @Override
    public void accept(String value) {}
    @Override
    public String name() {
        return /*@inherited_return*/null;
    }
    // 不是覆写：形参类型不同
    public void put(Integer value) {}
}

class Explicit extends Base {
    @Override
    public @Nullable String name() { return null; }
}

class Length implements Fn<String, Integer> {
    @Override
    public Integer apply(String input) {
        return /*@inherited_param*/input.length();
    }
}

class Checked implements Fn<String, Integer> {
    @Override
    public Integer apply(String input) {
        return input == null ? 0 : input.length();
    }
}

class RepoImpl implements Repo {
    @Override
    public Object load(String key) { return null; }
    @Override
    public void save(Object value) { value.hashCode(); }
}

interface First { @Nullable String get(); }
interface Second { @NotNull String get(); }

// 两个被覆写的方法上的注解互相矛盾：拿不准
class Both implements First, Second {
    @Override
    public String get() { return null; }
}

class Main {
    int use(Sub sub, RepoImpl repo, Both both) {
        int n = sub./*@inherited_deref*/find("k").length();
        sub.accept(/*@inherited_arg*/null);
        sub.put((Integer) null);
        n += repo./*@source_deref*/load("k").hashCode();
        repo.save(/*@source_arg*/null);
        n += both.get().length();
        String s = sub.find("x");
        if (s != null) {
            n += s.length();
        }
        return n;
    }
}
//? diag nullability-violation inherited_return
//? diag nullability-violation inherited_arg
//? diag nullability-violation source_arg
//? diag only nullability-violation
//? diag nullable-dereference inherited_param
//? diag nullable-dereference inherited_deref
//? diag nullable-dereference source_deref
//? diag only nullable-dereference
//? diag only null-dereference
//? diag only constant-condition
