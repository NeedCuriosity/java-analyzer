class ObjectCheck<T> {
    ObjectCheck<T> isNotNull() { return this; }
}

class ThrowableCheck<T extends Throwable> {
    ThrowableCheck<T> hasCauseInstanceOf(Class<? extends Throwable> type) { return this; }
}

class ComparableCheck<T> {
    ComparableCheck<T> isLessThan(T other) { return this; }
}

// 新版本：两个重载都是泛型方法，上界更窄的更具体
class Checks {
    static <T> ObjectCheck<T> check(T actual) { return new ObjectCheck<>(); }

    static <T extends Throwable> ThrowableCheck<T> check(T actual) { return new ThrowableCheck<>(); }

    static <T extends Comparable<? super T>> ComparableCheck<T> check(T actual) { return new ComparableCheck<>(); }
}

// 旧版本：非泛型的 `Throwable` 重载和泛型的 `T` 重载
class OldChecks {
    static <T> ObjectCheck<T> check(T actual) { return new ObjectCheck<>(); }

    static ThrowableCheck<Throwable> check(Throwable actual) { return new ThrowableCheck<>(); }

    static <T extends Comparable<? super T>> ComparableCheck<T> check(T actual) { return new ComparableCheck<>(); }
}

class Use {
    void run() {
        final Exception exception = new Exception();
        Checks.check(exception).hasCauseInstanceOf(IllegalStateException.class);
        OldChecks.check(exception).hasCauseInstanceOf(IllegalStateException.class);
        Checks.check("text").isLessThan("z");
        OldChecks.check("text").isLessThan("z");
        Checks.check(new Object()).isNotNull();
        OldChecks.check(new Object()).isNotNull();
    }
}
//? diag none
