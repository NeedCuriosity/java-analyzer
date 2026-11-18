//- /lib/Check.java
package lib;

import java.util.List;
import java.util.function.Function;

public class Check<A> {
    public static <A> Check<A> of(A actual) { return new Check<>(); }

    // 公开的可变参数重载
    @SafeVarargs
    public final ListCheck extracting(Function<? super A, ?>... extractors) { return new ListCheck(); }

    // 别的包里经由 `Check<Holder>` 访问不到的 protected 重载，不能被选中
    protected <T, C extends Check<T>> C extracting(Function<? super A, ? extends T> extractor, Factory<T, C> factory) {
        return null;
    }

    public interface Factory<T, C extends Check<T>> {
        C create(T actual);
    }

    public static class ListCheck {
        public ListCheck containsExactly(Object... values) { return this; }
    }

    protected void hook() {}
}
//- /app/Use.java
package app;

import lib.Check;

class Use {
    static class Inner { int identifier; }
    static class Holder { Inner first; Inner second; }

    void run(Holder holder) {
        Check.of(holder)
                .extracting(h -> h.first.identifier, h -> h.second.identifier)
                .containsExactly(42, 42);
    }

    // 匿名子类里经由 `super` 调用父类在别的包里的 protected 重载
    static ClassLoader wrap(ClassLoader parent) {
        return new ClassLoader(parent) {
            @Override
            protected Class<?> loadClass(String name, boolean resolve) throws ClassNotFoundException {
                return super.loadClass(name, resolve);
            }
        };
    }

    static Check<String> local() {
        return new Check<String>() {
            @Override
            protected void hook() {
                Check.Factory<Integer, Check<Integer>> factory = Check::of;
                Check<Integer> length = super.extracting(String::length, factory);
            }
        };
    }
}

class Sub extends Check<String> {
    @Override
    protected void hook() {
        super.hook();
        Check.Factory<Integer, Check<Integer>> factory = Check::of;
        Check<Integer> length = super.extracting(String::length, factory);
    }
}
//? diag none
