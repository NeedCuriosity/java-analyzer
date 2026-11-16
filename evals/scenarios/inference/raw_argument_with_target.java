import java.util.AbstractList;
import java.util.ArrayList;
import java.util.List;
import java.util.Optional;

class Main {
    /** 原始类型实现的列表（像 commons-collections 的 `TreeList`）。 */
    static class TreeList extends AbstractList {
        public Object get(int index) {
            return null;
        }

        public int size() {
            return 0;
        }
    }

    static class NewResponse<T> {
        static <T> NewResponse<T> success(T data) {
            return new NewResponse<>();
        }
    }

    record Brand(String name) {}

    NewResponse<List<Brand>> listByIds(List<Long> ids) {
        return NewResponse.success(new TreeList());
    }

    NewResponse<List<Brand>> fromRawList(List raw) {
        return NewResponse.success(raw);
    }

    NewResponse<List<Brand>> assigned() {
        NewResponse<List<Brand>> response = NewResponse.success(new ArrayList());
        return response;
    }

    Optional<List<String>> optional() {
        return Optional.of(new TreeList());
    }

    int noTarget() {
        var /*@inferred*/response = NewResponse.success(new TreeList());
        return response.hashCode();
    }

    NewResponse<String> wrong() {
        return /*@mismatch*/NewResponse.success(Integer.valueOf(1));
    }
}
//? hover inferred ~ "NewResponse<Main.TreeList>"
//? diag type-mismatch mismatch
//? diag exact
