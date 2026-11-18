// 和 AWS SDK v2 一样：`PutRequest.Builder` 从 `Request.Builder` 继承 `Request build()`，
// 又从 `SdkBuilder<Builder, PutRequest>` 继承 `PutRequest build()`，调用得到返回类型更具体的那个
interface SdkBuilder<B extends SdkBuilder<B, T>, T> { T build(); }

interface CopyableBuilder<B extends CopyableBuilder<B, T>, T> extends SdkBuilder<B, T> {}

abstract class Request {
    interface Builder { Request build(); }
}

class PutRequest extends Request {
    interface Builder extends Request.Builder, CopyableBuilder<Builder, PutRequest> {
        Builder bucket(String bucket);
    }

    static Builder builder() { return null; }
}

// 子接口用类型变量覆写父接口的方法时，子接口的版本才是继承下来的那个
interface Node<T> {
    Iterable<T> items();

    interface OfInt<T, S extends Iterable<T>, N extends OfInt<T, S, N>> extends Node<T> {
        @Override
        S items();
    }
}

class Use {
    PutRequest put() {
        var request = PutRequest.builder().bucket("b").build();
        /*@request*/request.hashCode();
        return PutRequest.builder().bucket("a").build();
    }

    <T, S extends Iterable<T>, N extends Node.OfInt<T, S, N>> S items(N node) {
        S items = node.items();
        return items;
    }
}
//? hover request ~ "PutRequest request"
//? diag none
