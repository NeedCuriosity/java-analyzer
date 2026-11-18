import java.util.Optional;
import java.util.function.Consumer;
import java.util.function.Supplier;

interface Propagator<T> {
    Supplier<Optional<T>> retrieve();
    Consumer<Optional<T>> copy();
}

interface NamedPropagator<T> extends Propagator<T> {
    String name();
}

// 实现原始类型：继承来的成员签名是擦除后的 `Supplier retrieve()`，返回 `Supplier<Optional>` 可以
class RawPropagator implements Propagator {
    public Supplier<Optional> retrieve() { return () -> Optional.empty(); }
    public Consumer<Optional> copy() { return t -> {}; }
}

// 经由原始类型间接继承的父接口，成员同样擦除
class RawNamed implements NamedPropagator {
    public Supplier<Optional> retrieve() { return () -> Optional.empty(); }
    public Consumer<Optional> copy() { return t -> {}; }
    public String name() { return "raw"; }
}

abstract class RawBase implements Propagator {}

class RawChild extends RawBase {
    public Supplier<Optional> retrieve() { return () -> Optional.empty(); }
    public Consumer<Optional> copy() { return t -> {}; }
}

// 擦除之后返回类型仍然不兼容
class /*@bad_class*/BadRaw implements Propagator {
    public Optional /*@bad_ret*/retrieve() { return Optional.empty(); }
    public Consumer<Optional> copy() { return t -> {}; }
}
//? diag invalid-override bad_ret
//? diag missing-implementation bad_class
//? diag exact
