//- /client/Mono.java
package client;

import java.util.function.Function;

public abstract class Mono<T> {
    public abstract <R> Mono<R> map(Function<? super T, ? extends R> mapper);
    public abstract T block();
}
//- /client/WebClient.java
package client;

public interface WebClient {
    RequestHeadersUriSpec<?> get();
    RequestBodyUriSpec post();

    interface Builder {
        WebClient build();
    }

    interface UriSpec<S extends RequestHeadersSpec<?>> {
        S uri(String uri, Object... uriVariables);
    }

    interface RequestHeadersSpec<S extends RequestHeadersSpec<S>> {
        S accept(String... mediaTypes);
        S header(String name, String... values);
        ResponseSpec retrieve();
    }

    interface RequestHeadersUriSpec<S extends RequestHeadersSpec<S>>
            extends UriSpec<S>, RequestHeadersSpec<S> {}

    interface RequestBodySpec extends RequestHeadersSpec<RequestBodySpec> {
        RequestHeadersSpec<?> bodyValue(Object body);
    }

    interface RequestBodyUriSpec extends RequestBodySpec, RequestHeadersUriSpec<RequestBodySpec> {}

    interface ResponseSpec {
        <T> Mono<T> bodyToMono(Class<T> elementClass);
    }
}
//- /client/RestClient.java
package client;

public interface RestClient {
    RequestHeadersUriSpec<?> get();

    interface UriSpec<S extends RequestHeadersSpec<?>> {
        S uri(String uri, Object... uriVariables);
    }

    interface RequestHeadersSpec<S extends RequestHeadersSpec<S>> {
        S accept(String... mediaTypes);
        ResponseSpec retrieve();
        ExchangeResult exchange();
    }

    interface RequestHeadersUriSpec<S extends RequestHeadersSpec<S>>
            extends UriSpec<S>, RequestHeadersSpec<S> {}

    interface ResponseSpec {
        <T> T body(Class<T> bodyType);
    }

    interface ExchangeResult {
        int status();
    }
}
//- /app/Owner.java
package app;

public class Owner {
    private String name;

    public String getName() {
        return name;
    }
}
//- /app/OwnerClient.java
package app;

import client.Mono;
import client.RestClient;
import client.WebClient;

class OwnerClient {
    Mono<Owner> find(WebClient.Builder builder, int id) {
        var /*@mono*/mono = builder.build().get().uri("http://owners/{id}", id).retrieve().bodyToMono(Owner.class);
        var /*@names*/names = builder.build().get().uri("http://owners").accept("application/json")
                .header("X-Trace", "1").retrieve().bodyToMono(Owner.class).map(Owner::getName);
        var /*@posted*/posted = builder.build().post().uri("http://owners").bodyValue(id).retrieve().bodyToMono(Owner.class);
        System.out.println("" + names + posted);
        return mono;
    }

    String name(RestClient rest, int id) {
        Owner /*@owner*/owner = rest.get().uri("http://owners/{id}", id).accept("application/json").retrieve().body(Owner.class);
        int /*@status*/status = rest.get().uri("http://owners").exchange().status();
        rest.get().uri("http://owners")./*@members*/retrieve();
        return owner.getName() + status;
    }
}
//? hover mono ~ "Mono<Owner> mono"
//? hover names ~ "Mono<String> names"
//? hover posted ~ "Mono<Owner> posted"
//? complete members has accept retrieve exchange
//? diag none
