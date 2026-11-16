import java.util.List;

abstract class TypeRef<T> {
    T describe() { return null; }
}

class Client {
    <T> T body(TypeRef<T> type) { return null; }
}

class Main {
    List<String> names(Client client) {
        return client.body(/*@arg*/new TypeRef<>() {
        });
    }
}
//? hover arg ~ "TypeRef<List<String>>"
//? diag none
