//- /org/data/CrudRepository.java lib:data
package org.data;

import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.function.Function;

public interface CrudRepository<T, ID> {
    Optional<T> findById(ID id);
    List<T> findAll();
    Map<ID, List<T>> grouped(Function<? super T, ? extends ID> key);
    <S extends T> S save(S entity) throws java.io.IOException;
    Map.Entry<ID, T> first();
    long count();
}
//- /org/data/Page.java lib:data
package org.data;

import java.util.Iterator;
import java.util.concurrent.atomic.AtomicInteger;

public abstract class Page<T> implements Iterable<T>, java.io.Serializable {
    public static final int DEFAULT_SIZE = 20;
    protected final AtomicInteger counter = new AtomicInteger();

    public enum Direction { ASC, DESC }

    public static class Order {
        public Direction direction;
        public Order(Direction direction) { this.direction = direction; }
    }

    public abstract Iterator<T> iterator();
    public Order[] orders() { return new Order[0]; }
}
//- /app/Main.java
package app;

import org.data.CrudRepository;
import org.data.Page;

class Main {
    void run(CrudRepository<String, Long> repository, Page<String> page) {
        repository./*@call*/findById(1L);
        page.orders();
    }
}
//? stub org.data.CrudRepository ; has "interface CrudRepository<T, ID>" ; has "findById(" ; has "grouped("
