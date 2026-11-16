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
//- /app/Main.java
package app;

import org.jetbrains.annotations.NotNull;
import org.jetbrains.annotations.Nullable;

class Main {
    @Nullable String name;
    @Nullable Main next;
    static @Nullable Main instance;

    int direct() {
        return /*@direct*/name.length();
    }

    int qualified() {
        return this./*@qualified*/name.length();
    }

    int guarded() {
        if (name != null) {
            return name.length();
        }
        if (this.next == null) {
            return 0;
        }
        return next.hashCode() + this.next.hashCode();
    }

    int elseBranch(boolean flag) {
        if (flag && name != null) {
            return name.length();
        } else {
            return /*@else_branch*/name.length();
        }
    }

    int assigned() {
        name = "x";
        int n = name.length();
        name = null;
        return n + /*@assigned_null*/name.length();
    }

    int checkedAfterUse() {
        int n = /*@before_check*/name.length();
        if (name != null) {
            n++;
        }
        return n;
    }

    int afterCall() {
        if (name != null) {
            log();
            // 调用可能改掉了字段：拿不准，不报
            return name.length();
        }
        return 0;
    }

    int lazy() {
        if (instance == null) {
            instance = new Main();
        }
        if (Main.instance != null) {
            return instance.hashCode();
        }
        return instance.hashCode();
    }

    int lambda() {
        if (name != null) {
            Runnable r = () -> name.length();
            r.run();
        }
        return 0;
    }

    int asserted() {
        assert name != null;
        return name.length();
    }

    int precondition() {
        checkState();
        return name.length();
    }

    int predicate() {
        if (!hasName()) {
            return 0;
        }
        return name.length();
    }

    int loop() {
        int n = 0;
        while (next != null) {
            n += next.hashCode();
            next = next.next;
        }
        for (Main cur = next; cur != null; cur = cur.next) {
            n++;
        }
        return n;
    }

    int other(Main other) {
        // 别的对象上的字段不跟踪
        return other.name.length();
    }

    int viaLocal() {
        String s = name;
        return /*@via_local*/s.length();
    }

    @NotNull String returned() {
        return /*@returned*/name;
    }

    void passed() {
        take(/*@passed*/name);
        if (name != null) {
            take(name);
        }
    }

    void log() {}
    void checkState() {}
    boolean hasName() { return name != null; }
    void take(@NotNull String s) {}
}
//? diag nullable-dereference direct
//? diag nullable-dereference qualified
//? diag nullable-dereference else_branch
//? diag nullable-dereference assigned_null
//? diag nullable-dereference before_check
//? diag nullable-dereference via_local
//? diag only nullable-dereference
//? diag nullability-violation returned
//? diag nullability-violation passed
//? diag only nullability-violation
//? diag only null-dereference
//? diag only constant-condition
